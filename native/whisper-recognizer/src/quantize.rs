//! Bit-preserving acceleration of tract's dynamic f32 -> u8 quantizer.
//! Preserve division, ties-to-even rounding, separate zero-point addition and
//! saturation. Reciprocal multiplication and rounding after adding zero point
//! can change int8 activations, so neither is used here.
use rayon::prelude::*;
use tract_linalg::multithread::{current_tract_executor, Executor};
use tract_onnx::tract_core::internal::*;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct DynamicQuantize;
impl Op for DynamicQuantize {
    fn name(&self) -> StaticName {
        "JarvisDynamicQuantize".into()
    }
    op_as_typed_op!();
}
impl EvalOp for DynamicQuantize {
    op_out_of_plan!();
    fn eval(&self, _: &EvalContext, inputs: TVec<TValue>) -> TractResult<TVec<TValue>> {
        let input = args_1!(inputs);
        let source = input.try_as_plain_ram()?.as_slice::<f32>()?;
        let (min, max) = minmax(source);
        let scale = (max - min) / 255.0;
        // max/min deliberately handle NaN like the reference's zero-range case.
        #[allow(clippy::manual_clamp)]
        let zero = (-min / scale).round_ties_even().max(0.0).min(255.0) as u8;
        let mut output = Tensor::zero::<u8>(input.shape())?;
        let mut plain = output.try_as_plain_ram_mut()?;
        let destination = plain.as_slice_mut::<u8>()?;
        match current_tract_executor() {
            Executor::MultiThread(pool)
                if source.len() >= 65536 && pool.current_num_threads() > 1 =>
            {
                pool.install(|| {
                    source
                        .par_chunks(32768)
                        .zip(destination.par_chunks_mut(32768))
                        .for_each(|(s, d)| convert(s, d, scale, zero))
                });
            }
            _ => convert(source, destination, scale, zero),
        }
        Ok(tvec!(
            output.into_tvalue(),
            tensor0(scale).into_tvalue(),
            tensor0(zero).into_tvalue()
        ))
    }
}
impl TypedOp for DynamicQuantize {
    fn output_facts(&self, inputs: &[&TypedFact]) -> TractResult<TVec<TypedFact>> {
        ensure!(
            inputs.len() == 1 && inputs[0].datum_type == DatumType::F32,
            "Expected f32 activations"
        );
        Ok(tvec!(
            u8::fact(inputs[0].shape.clone()),
            f32::fact([0; 0]),
            u8::fact([0; 0])
        ))
    }
    as_op!();
}
pub fn install(mut model: TypedModel) -> TractResult<TypedModel> {
    let targets: Vec<_> = model
        .nodes()
        .iter()
        .filter(|n| n.op.name() == "DynamicQuantizeLinearU8")
        .map(|n| n.id)
        .collect();
    for id in targets {
        let node = model.node(id);
        TypedModelPatch::replace_single_op(&model, node, &node.inputs, DynamicQuantize)?
            .apply(&mut model)?;
    }
    model.into_compact()
}
fn scalar_minmax(source: &[f32]) -> (f32, f32) {
    source.iter().fold((0.0, 0.0), |(min, max), &v| {
        if v < min {
            (v, max)
        } else if v > max {
            (min, v)
        } else {
            (min, max)
        }
    })
}
fn scalar_convert(source: &[f32], destination: &mut [u8], scale: f32, zero: u8) {
    for (&x, y) in source.iter().zip(destination) {
        *y = (((x / scale).round_ties_even() as i32) + zero as i32).clamp(0, 255) as u8;
    }
}
fn minmax(source: &[f32]) -> (f32, f32) {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        // SAFETY: runtime feature check; the implementation reads complete
        // eight-element chunks and handles the remainder separately.
        return unsafe { avx2_minmax(source) };
    }
    scalar_minmax(source)
}
fn convert(source: &[f32], destination: &mut [u8], scale: f32, zero: u8) {
    assert_eq!(source.len(), destination.len());
    #[cfg(target_arch = "x86_64")]
    if scale.is_normal() && std::is_x86_feature_detected!("avx2") {
        // SAFETY: checked feature, equal lengths, bounded full-width loads and
        // stores. Exceptional scales use the exact portable reference path.
        unsafe { avx2_convert(source, destination, scale, zero) };
        return;
    }
    scalar_convert(source, destination, scale, zero);
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_minmax(source: &[f32]) -> (f32, f32) {
    use std::arch::x86_64::*;
    let mut min = _mm256_setzero_ps();
    let mut max = min;
    let (chunks, tail) = source.as_chunks::<8>();
    for chunk in chunks {
        // SAFETY: chunk contains eight contiguous f32 values. MIN/MAX put the
        // previous accumulator second, so a NaN source is ignored as in tract.
        let x = unsafe { _mm256_loadu_ps(chunk.as_ptr()) };
        min = _mm256_min_ps(x, min);
        max = _mm256_max_ps(x, max);
    }
    let mut lows = [0.0; 8];
    let mut highs = [0.0; 8];
    unsafe {
        _mm256_storeu_ps(lows.as_mut_ptr(), min);
        _mm256_storeu_ps(highs.as_mut_ptr(), max);
    }
    let (mut low, mut high) = scalar_minmax(&lows);
    for &x in highs.iter().chain(tail) {
        if x < low {
            low = x;
        } else if x > high {
            high = x;
        }
    }
    (low, high)
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_convert(source: &[f32], destination: &mut [u8], scale: f32, zero: u8) {
    use std::arch::x86_64::*;
    let scales = _mm256_set1_ps(scale);
    let zeros = _mm256_set1_epi32(zero as i32);
    let low = _mm256_setzero_si256();
    let high = _mm256_set1_epi32(255);
    let count = source.len() / 8 * 8;
    for offset in (0..count).step_by(8) {
        // SAFETY: count excludes the tail; the u64 store writes exactly eight
        // u8 outputs. Floating-point division is not replaced with reciprocal.
        unsafe {
            let x = _mm256_loadu_ps(source.as_ptr().add(offset));
            let divided = _mm256_div_ps(x, scales);
            let ordered = _mm256_cmp_ps::<_CMP_ORD_Q>(divided, divided);
            let divided = _mm256_and_ps(divided, ordered); // NaN casts to zero in Rust.
            let rounded =
                _mm256_round_ps::<{ _MM_FROUND_TO_NEAREST_INT | _MM_FROUND_NO_EXC }>(divided);
            let q = _mm256_add_epi32(_mm256_cvttps_epi32(rounded), zeros);
            let q = _mm256_min_epi32(high, _mm256_max_epi32(low, q));
            let packed =
                _mm_packus_epi32(_mm256_castsi256_si128(q), _mm256_extracti128_si256::<1>(q));
            let bytes = _mm_packus_epi16(packed, _mm_setzero_si128());
            _mm_storel_epi64(destination.as_mut_ptr().add(offset).cast(), bytes);
        }
    }
    scalar_convert(&source[count..], &mut destination[count..], scale, zero);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tract_onnx::prelude::InferenceModelExt;
    fn reference(input: TValue) -> TractResult<TVec<TValue>> {
        use tract_onnx::pb;
        let value = |name: &str, elem_type: i32| pb::ValueInfoProto {
            name: name.into(),
            r#type: Some(pb::TypeProto {
                value: Some(pb::type_proto::Value::TensorType(pb::type_proto::Tensor {
                    elem_type,
                    shape: None,
                })),
                ..Default::default()
            }),
            ..Default::default()
        };
        let proto = pb::ModelProto {
            ir_version: 8,
            opset_import: vec![pb::OperatorSetIdProto {
                domain: String::new(),
                version: 13,
            }],
            graph: Some(pb::GraphProto {
                input: vec![value("x", 1)],
                output: vec![value("y", 2), value("scale", 1), value("zero", 2)],
                node: vec![pb::NodeProto {
                    name: "reference".into(),
                    op_type: "DynamicQuantizeLinear".into(),
                    input: vec!["x".into()],
                    output: vec!["y".into(), "scale".into(), "zero".into()],
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut model = tract_onnx::onnx().model_for_proto_model(&proto)?;
        model.set_input_fact(0, f32::fact(input.shape()).into())?;
        model.into_optimized()?.into_runnable()?.run(tvec!(input))
    }
    fn compare(data: &[f32]) -> TractResult<()> {
        let input = Tensor::from_shape(&[data.len()], data)?.into_tvalue();
        let context = EvalContext::out_of_plan();
        let expected = reference(input.clone())?;
        let actual = DynamicQuantize.eval(&context, tvec!(input))?;
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.as_bytes(), expected.as_bytes());
        }
        let (min, max) = scalar_minmax(data);
        let scale = (max - min) / 255.0;
        #[allow(clippy::manual_clamp)]
        let zero = (-min / scale).round_ties_even().max(0.0).min(255.0) as u8;
        let mut scalar = vec![0; data.len()];
        scalar_convert(data, &mut scalar, scale, zero);
        assert_eq!(
            scalar.as_slice(),
            expected[0].try_as_plain_ram()?.as_slice::<u8>()?
        );
        assert_eq!(scale.to_le_bytes(), expected[1].as_bytes());
        assert_eq!([zero], expected[2].as_bytes());
        Ok(())
    }
    #[test]
    fn matches_original_on_random_activations_and_unaligned_tails() -> TractResult<()> {
        let mut seed = 0x91ab_47d3_u32;
        for len in [0, 1, 7, 8, 9, 255, 768, 32768, 65539, 1_000_003] {
            let data: Vec<f32> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as i32 as f32) / 1234567.0
                })
                .collect();
            compare(&data)?;
        }
        Ok(())
    }
    #[test]
    fn matches_original_at_rounding_boundaries_and_exceptional_ranges() -> TractResult<()> {
        compare(&[-127., 128., -2.5, -1.5, -0.5, 0.5, 1.5, 2.5, 3.5, f32::NAN])?;
        compare(&[
            f32::NAN,
            -127.0,
            f32::NAN,
            128.0,
            -1.5,
            0.5,
            2.5,
            f32::NAN,
            17.5,
        ])?;
        let mut boundaries = vec![-127.0, 128.0];
        for value in [-126.5_f32, -2.5, -1.5, -0.5, 0.5, 1.5, 2.5, 126.5] {
            for delta in [-1, 0, 1] {
                boundaries.push(f32::from_bits(value.to_bits().wrapping_add_signed(delta)));
            }
        }
        compare(&boundaries)?;
        for data in [
            vec![0.; 17],
            vec![-0.; 17],
            vec![f32::NAN; 17],
            vec![f32::INFINITY; 17],
            vec![f32::NEG_INFINITY; 17],
            vec![f32::from_bits(1); 17],
            vec![-f32::MAX, f32::MAX, 0.],
        ] {
            compare(&data)?;
        }
        Ok(())
    }
    #[test]
    fn parallel_execution_uses_identical_global_scale() -> TractResult<()> {
        let source: Vec<f32> = (0..200003)
            .map(|i| (i % 10000) as f32 / 251.0 - 13.0)
            .collect();
        tract_linalg::multithread::multithread_tract_scope(Executor::multithread(3), || {
            compare(&source)
        })
    }
}
