//! Lower one decoder block into ordinary ONNX primitives for tract's Rust
//! kernels. Cache ownership is in Rust; exported ScatterND/index-building
//! subgraphs are replaced by bounded append-only per-command KV tensors.
use std::collections::HashMap;
use tract_onnx::tract_core::internal::{ensure, format_err, DimLike};
use tract_onnx::{pb, prelude::*};

pub struct Builder<'a> {
    graph: pb::GraphProto,
    source: &'a pb::GraphProto,
    weights: HashMap<&'a str, &'a pb::TensorProto>,
    count: usize,
}
fn value(name: &str) -> pb::ValueInfoProto {
    pb::ValueInfoProto {
        name: name.into(),
        r#type: Some(pb::TypeProto {
            value: Some(pb::type_proto::Value::TensorType(pb::type_proto::Tensor {
                elem_type: 1,
                shape: None,
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn attr_i(name: &str, i: i64) -> pb::AttributeProto {
    pb::AttributeProto {
        name: name.into(),
        r#type: 2,
        i,
        ..Default::default()
    }
}
fn attr_is(name: &str, ints: &[i64]) -> pb::AttributeProto {
    pb::AttributeProto {
        name: name.into(),
        r#type: 7,
        ints: ints.into(),
        ..Default::default()
    }
}
impl<'a> Builder<'a> {
    pub fn new(source: &'a pb::GraphProto) -> Self {
        Self {
            graph: Default::default(),
            source,
            weights: source
                .initializer
                .iter()
                .map(|t| (t.name.as_str(), t))
                .collect(),
            count: 0,
        }
    }
    fn weight(&mut self, name: &str) -> TractResult<String> {
        if !self.graph.initializer.iter().any(|t| t.name == name) {
            self.graph.initializer.push(
                (*self
                    .weights
                    .get(name)
                    .ok_or_else(|| format_err!("Missing weight {name}"))?)
                .clone(),
            );
        }
        Ok(name.into())
    }
    fn constant_f32(&mut self, value: f32) -> String {
        let name = format!("constant_{}", self.count);
        self.count += 1;
        self.graph.initializer.push(pb::TensorProto {
            name: name.clone(),
            data_type: 1,
            float_data: vec![value],
            ..Default::default()
        });
        name
    }
    fn shape(&mut self, values: &[i64]) -> String {
        let name = format!("shape_{}", self.count);
        self.count += 1;
        self.graph.initializer.push(pb::TensorProto {
            name: name.clone(),
            data_type: 7,
            dims: vec![values.len() as i64],
            int64_data: values.into(),
            ..Default::default()
        });
        name
    }
    fn op(&mut self, op: &str, inputs: &[&str], attrs: Vec<pb::AttributeProto>) -> String {
        let name = format!("{op}_{}", self.count);
        self.count += 1;
        self.graph.node.push(pb::NodeProto {
            name: name.clone(),
            op_type: op.into(),
            input: inputs.iter().map(|s| (*s).into()).collect(),
            output: vec![name.clone()],
            attribute: attrs,
            ..Default::default()
        });
        name
    }
    fn norm(&mut self, x: &str, prefix: &str) -> TractResult<String> {
        let mean = self.op("ReduceMean", &[x], vec![attr_is("axes", &[-1])]);
        let sub = self.op("Sub", &[x, &mean], vec![]);
        let two = self.constant_f32(2.0);
        let pow = self.op("Pow", &[&sub, &two], vec![]);
        let var = self.op("ReduceMean", &[&pow], vec![attr_is("axes", &[-1])]);
        let eps = self.constant_f32(1e-5);
        let var = self.op("Add", &[&var, &eps], vec![]);
        let std = self.op("Sqrt", &[&var], vec![]);
        let norm = self.op("Div", &[&sub, &std], vec![]);
        let weight = self.weight(&format!("{prefix}.weight"))?;
        let bias = self.weight(&format!("{prefix}.bias"))?;
        let scaled = self.op("Mul", &[&norm, &weight], vec![]);
        Ok(self.op("Add", &[&scaled, &bias], vec![]))
    }
    fn linear(&mut self, x: &str, name: &str) -> TractResult<String> {
        let node = self
            .source
            .node
            .iter()
            .find(|n| n.name == format!("{name}/MatMul_quant"))
            .ok_or_else(|| format_err!("Missing linear {name}"))?;
        ensure!(
            node.op_type == "MatMulInteger" && node.input.len() == 4,
            "Unsupported linear {name}"
        );
        let w = self.weight(&node.input[1])?;
        let zero = self.weight(&node.input[3])?;
        let scale = self.weight(&node.input[1].replace("_quantized", "_scale"))?;
        let q = format!("quant_{}", self.count);
        self.count += 1;
        let qs = format!("{q}_scale");
        let qz = format!("{q}_zero");
        self.graph.node.push(pb::NodeProto {
            name: q.clone(),
            op_type: "DynamicQuantizeLinear".into(),
            input: vec![x.into()],
            output: vec![q.clone(), qs.clone(), qz.clone()],
            ..Default::default()
        });
        let raw = self.op("MatMulInteger", &[&q, &w, &qz, &zero], vec![]);
        let raw = self.op("Cast", &[&raw], vec![attr_i("to", 1)]);
        let scale = self.op("Mul", &[&qs, &scale], vec![]);
        let y = self.op("Mul", &[&raw, &scale], vec![]);
        if let Some(add) = self
            .source
            .node
            .iter()
            .find(|n| n.name == format!("{name}/Add"))
        {
            let name = add
                .input
                .iter()
                .find(|s| self.weights.contains_key(s.as_str()))
                .ok_or_else(|| format_err!("Missing linear bias {name}"))?
                .clone();
            let bias = self.weight(&name)?;
            Ok(self.op("Add", &[&bias, &y], vec![]))
        } else {
            Ok(y)
        }
    }
    fn attention(&mut self, q: &str, k: &str, v: &str, mask: Option<&str>) -> String {
        let shape = self.shape(&[1, -1, 12, 64]);
        let scale = self.constant_f32(64_f32.powf(-0.25));
        let q = self.op("Reshape", &[q, &shape], vec![]);
        let q = self.op("Transpose", &[&q], vec![attr_is("perm", &[0, 2, 1, 3])]);
        let q = self.op("Mul", &[&q, &scale], vec![]);
        let k = self.op("Reshape", &[k, &shape], vec![]);
        let k = self.op("Transpose", &[&k], vec![attr_is("perm", &[0, 2, 3, 1])]);
        let k = self.op("Mul", &[&k, &scale], vec![]);
        let scores = self.op("MatMul", &[&q, &k], vec![]);
        let scores = if let Some(mask) = mask {
            self.op("Add", &[&scores, mask], vec![])
        } else {
            scores
        };
        let probs = self.op("Softmax", &[&scores], vec![attr_i("axis", -1)]);
        let v = self.op("Reshape", &[v, &shape], vec![]);
        let v = self.op("Transpose", &[&v], vec![attr_is("perm", &[0, 2, 1, 3])]);
        let context = self.op("MatMul", &[&probs, &v], vec![]);
        let context = self.op(
            "Transpose",
            &[&context],
            vec![attr_is("perm", &[0, 2, 1, 3])],
        );
        let shape = self.shape(&[1, -1, 768]);
        self.op("Reshape", &[&context, &shape], vec![])
    }
    fn finish(
        mut self,
        inputs: &[(&str, TypedFact)],
        outputs: &[String],
        scope: SymbolScope,
    ) -> TractResult<TypedModel> {
        self.graph.input = inputs.iter().map(|(name, _)| value(name)).collect();
        self.graph.output = outputs.iter().map(|name| value(name)).collect();
        let proto = pb::ModelProto {
            ir_version: 8,
            opset_import: vec![pb::OperatorSetIdProto {
                domain: String::new(),
                version: 13,
            }],
            graph: Some(self.graph),
            ..Default::default()
        };
        let mut model = tract_onnx::onnx().model_for_proto_model(&proto)?;
        model.symbols = scope;
        for (i, (_, fact)) in inputs.iter().enumerate() {
            model.set_input_fact(i, fact.clone().into())?;
        }
        model.into_optimized()
    }
    pub fn block(mut self, index: usize) -> TractResult<TypedModel> {
        let suffix = |i: usize| {
            if i == 0 {
                String::new()
            } else {
                format!("_{i}")
            }
        };
        let prefix = format!("textDecoder.blocks.{index}");
        let x = self.norm("x", &format!("{prefix}.attn_ln"))?;
        let q = self.linear(&x, &format!("/query{}", suffix(index * 2)))?;
        let k = self.linear(&x, &format!("/key{}", suffix(index)))?;
        let v = self.linear(&x, &format!("/value{}", suffix(index)))?;
        let k = self.op("Concat", &["old_k", &k], vec![attr_i("axis", 1)]);
        let v = self.op("Concat", &["old_v", &v], vec![attr_i("axis", 1)]);
        let attn = self.attention(&q, &k, &v, Some("mask"));
        let attn = self.linear(&attn, &format!("/out{}", suffix(index * 2)))?;
        let x = self.op("Add", &["x", &attn], vec![]);
        let norm = self.norm(&x, &format!("{prefix}.cross_attn_ln"))?;
        let q = self.linear(&norm, &format!("/query{}", suffix(index * 2 + 1)))?;
        let attn = self.attention(&q, "cross_k", "cross_v", None);
        let attn = self.linear(&attn, &format!("/out{}", suffix(index * 2 + 1)))?;
        let x = self.op("Add", &[&x, &attn], vec![]);
        let norm = self.norm(&x, &format!("{prefix}.mlp_ln"))?;
        let hidden = self.linear(&norm, &format!("/mlp/mlp.0{}", suffix(index)))?;
        let sqrt2 = self.constant_f32(std::f32::consts::SQRT_2);
        let div = self.op("Div", &[&hidden, &sqrt2], vec![]);
        let erf = self.op("Erf", &[&div], vec![]);
        let one = self.constant_f32(1.0);
        let erf = self.op("Add", &[&erf, &one], vec![]);
        let gelu = self.op("Mul", &[&hidden, &erf], vec![]);
        let half = self.constant_f32(0.5);
        let gelu = self.op("Mul", &[&gelu, &half], vec![]);
        let hidden = self.linear(&gelu, &format!("/mlp/mlp.2{}", suffix(index)))?;
        let x = self.op("Add", &[&x, &hidden], vec![]);
        let scope = SymbolScope::default();
        let n = scope.sym("N").to_dim();
        let p = scope.sym("P").to_dim();
        let t = scope.sym("T").to_dim();
        self.finish(
            &[
                ("x", f32::fact([1.to_dim(), n.clone(), 768.to_dim()])),
                ("old_k", f32::fact([1.to_dim(), p.clone(), 768.to_dim()])),
                ("old_v", f32::fact([1.to_dim(), p.clone(), 768.to_dim()])),
                ("cross_k", f32::fact([1.to_dim(), t.clone(), 768.to_dim()])),
                ("cross_v", f32::fact([1.to_dim(), t.clone(), 768.to_dim()])),
                (
                    "mask",
                    f32::fact([1.to_dim(), 1.to_dim(), n.clone(), p + n]),
                ),
            ],
            &[x, k, v],
            scope,
        )
    }
    pub fn final_norm(mut self) -> TractResult<TypedModel> {
        let y = self.norm("x", "textDecoder.ln")?;
        self.finish(
            &[("x", f32::fact([1, 1, 768]))],
            &[y],
            SymbolScope::default(),
        )
    }
}

/// Exported symbolic annotations incorrectly reuse T for encoder input/output.
/// Discard annotations and let tract infer shapes from the supplied inputs.
pub fn clear_shapes(proto: &mut pb::ModelProto) -> TractResult<()> {
    let graph = proto
        .graph
        .as_mut()
        .ok_or_else(|| format_err!("Missing graph"))?;
    graph.value_info.clear();
    for value in graph.input.iter_mut().chain(graph.output.iter_mut()) {
        if let Some(pb::type_proto::Value::TensorType(t)) =
            value.r#type.as_mut().and_then(|t| t.value.as_mut())
        {
            t.shape = None;
        }
    }
    Ok(())
}

/// Compatibility signature for the supported sherpa small.en export's wiring.
/// This is not a cryptographic integrity check and does not hash trained data.
pub fn check_export(graph: &pb::GraphProto, expected: u64) -> TractResult<()> {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for node in &graph.node {
        for text in [&node.name, &node.domain, &node.op_type]
            .into_iter()
            .map(String::as_str)
            .chain(node.input.iter().map(String::as_str))
            .chain(["->"])
            .chain(node.output.iter().map(String::as_str))
        {
            for byte in text.bytes().chain([0]) {
                hash = (hash ^ byte as u64).wrapping_mul(1_099_511_628_211);
            }
        }
    }
    ensure!(
        hash == expected,
        "Unsupported Whisper ONNX export wiring; expected the installed small.en int8 export"
    );
    Ok(())
}

/// Quantized matmul needs column sums of its constant weights to compensate
/// activation zero points. tract's 4 MiB constant-fold limit leaves large
/// i8->i32->sum chains at runtime. Fold only the small sum, discarding the
/// expanded temporary immediately. This removes repeated conversions and the
/// otherwise-retained duplicate un-packed MLP weights without changing math.
pub fn fold_weight_sums(mut model: TypedModel) -> TractResult<TypedModel> {
    use tract_onnx::tract_core::ops::{cast::Cast, konst::Const, nn::Reduce};
    let mut replacements = Vec::new();
    for node in model.nodes() {
        let Some(reduce) = node.op_as::<Reduce>() else {
            continue;
        };
        if node.inputs.len() != 1 || node.outputs.len() != 1 {
            continue;
        }
        let cast = model.node(node.inputs[0].node);
        let Some(op) = cast.op_as::<Cast>() else {
            continue;
        };
        if op.to != DatumType::I32 || cast.inputs.len() != 1 {
            continue;
        }
        let Some(weight) = &model.outlet_fact(cast.inputs[0])?.konst else {
            continue;
        };
        if weight.datum_type() != DatumType::I8
            || node.outputs[0]
                .fact
                .shape
                .volume()
                .to_usize()
                .unwrap_or(usize::MAX)
                > 16384
        {
            continue;
        }
        let expanded = weight.cast_to_dt(DatumType::I32)?;
        let sum = reduce.reducer.reduce(&reduce.axes, &expanded)?;
        replacements.push((node.id, sum));
    }
    for (id, value) in replacements {
        TypedModelPatch::replace_single_op(
            &model,
            model.node(id),
            &[],
            Const::new(value.into_arc_tensor())?,
        )?
        .apply(&mut model)?;
    }
    model.into_compact()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precomputing_large_weight_sums_preserves_exact_integer_results() -> TractResult<()> {
        use tract_onnx::tract_core::ops::{
            cast,
            nn::{Reduce, Reducer},
        };
        let mut model = TypedModel::default();
        let data: Vec<i8> = (0..1536 * 768)
            .map(|i| ((i * 17) % 255) as i16 as i8)
            .collect();
        let weight = model.add_const("weight", Tensor::from_shape(&[1536, 768], &data)?)?;
        let cast = model.wire_node("expanded", cast::cast(DatumType::I32), &[weight])?;
        let sum = model.wire_node(
            "sum",
            Reduce {
                axes: tvec!(0),
                reducer: Reducer::Sum,
            },
            &cast,
        )?;
        model.select_output_outlets(&sum)?;
        let expected = model.clone().into_runnable()?.run(tvec!())?;
        let folded = fold_weight_sums(model)?;
        assert_eq!(folded.nodes().len(), 1, "Expanded weight must be discarded");
        let actual = folded.into_runnable()?.run(tvec!())?;
        assert_eq!(expected[0].as_bytes(), actual[0].as_bytes());
        Ok(())
    }
}
