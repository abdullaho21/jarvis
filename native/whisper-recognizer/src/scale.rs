//! Contiguous scalar multiplication for Whisper's attention/dequant scales.
//! The generic broadcast iterator otherwise visits every element individually.
use rayon::prelude::*;
use tract_linalg::multithread::{current_tract_executor, Executor};
use tract_onnx::tract_core::{
    internal::*,
    ops::{binary::TypedBinOp, math::Mul},
};
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Scale {
    scalar: usize,
}
impl Op for Scale {
    fn name(&self) -> StaticName {
        "JarvisScale".into()
    }
    op_as_typed_op!();
}
// Preserve the original operand order, including NaN propagation.
#[allow(clippy::assign_op_pattern)]
fn multiply(values: &mut [f32], scale: f32, scalar_first: bool) {
    if scalar_first {
        for x in values {
            *x = scale * *x;
        }
    } else {
        for x in values {
            *x *= scale;
        }
    }
}
impl EvalOp for Scale {
    op_out_of_plan!();
    fn eval(&self, _: &EvalContext, inputs: TVec<TValue>) -> TractResult<TVec<TValue>> {
        let factor = inputs[self.scalar].cast_to_scalar::<f32>()?;
        let mut inputs = inputs.into_iter();
        let a = inputs.next().unwrap();
        let b = inputs.next().unwrap();
        let mut output = if self.scalar == 0 { b } else { a }.into_tensor();
        let mut plain = output.try_as_plain_ram_mut()?;
        let values = plain.as_slice_mut::<f32>()?;
        match current_tract_executor() {
            Executor::MultiThread(pool)
                if values.len() >= 65536 && pool.current_num_threads() > 1 =>
            {
                pool.install(|| {
                    values
                        .par_chunks_mut(32768)
                        .for_each(|v| multiply(v, factor, self.scalar == 0))
                });
            }
            _ => multiply(values, factor, self.scalar == 0),
        }
        Ok(tvec!(output.into_tvalue()))
    }
}
impl TypedOp for Scale {
    fn output_facts(&self, inputs: &[&TypedFact]) -> TractResult<TVec<TypedFact>> {
        ensure!(
            inputs.len() == 2
                && inputs.iter().all(|f| f.datum_type == DatumType::F32)
                && inputs[self.scalar].shape.volume() == 1.to_dim(),
            "Expected one scalar and one f32 tensor"
        );
        Ok(tvec!(f32::fact(inputs[1 - self.scalar].shape.clone())))
    }
    as_op!();
}
pub fn install(mut model: TypedModel) -> TractResult<TypedModel> {
    let mut replacements = Vec::new();
    for node in model.nodes() {
        let Some(binary) = node.op_as::<TypedBinOp>() else {
            continue;
        };
        if !binary.0.is::<Mul>() || node.outputs[0].fact.datum_type != DatumType::F32 {
            continue;
        }
        let facts = model.node_input_facts(node.id)?;
        if facts.len() != 2 || facts.iter().any(|f| f.datum_type != DatumType::F32) {
            continue;
        }
        if let Some(scalar) = facts.iter().position(|f| f.shape.volume() == 1.to_dim()) {
            replacements.push((node.id, scalar));
        }
    }
    for (id, scalar) in replacements {
        let node = model.node(id);
        TypedModelPatch::replace_single_op(&model, node, &node.inputs, Scale { scalar })?
            .apply(&mut model)?;
    }
    model.into_compact()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_original_broadcast_and_keeps_shared_input_unchanged() -> TractResult<()> {
        let original: Vec<f32> = (0..65541).map(|i| (i % 413) as f32 / 129.0 - 2.0).collect();
        let tensor = Tensor::from_shape(&[3, 21847], &original)?.into_tvalue();
        for scalar in [0, 1] {
            for factor in [-0.0, 0.35355338, 1.0, -3.7, f32::INFINITY] {
                let factor = Tensor::from_shape(&[1, 1], &[factor])?.into_tvalue();
                let args = if scalar == 0 {
                    tvec!(factor, tensor.clone())
                } else {
                    tvec!(tensor.clone(), factor)
                };
                let context = EvalContext::out_of_plan();
                let reference =
                    tract_onnx::tract_core::ops::math::mul().eval(&context, args.clone())?;
                let actual = Scale { scalar }.eval(&context, args)?;
                assert_eq!(actual[0].as_bytes(), reference[0].as_bytes());
                assert_eq!(
                    tensor.try_as_plain_ram()?.as_slice::<f32>()?,
                    original.as_slice()
                );
            }
        }
        Ok(())
    }
    #[test]
    fn parallel_scaling_matches_single_thread() -> TractResult<()> {
        tract_linalg::multithread::multithread_tract_scope(
            Executor::multithread(3),
            matches_original_broadcast_and_keeps_shared_input_unchanged,
        )
    }
}
