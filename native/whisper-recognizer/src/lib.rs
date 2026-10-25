//! Whisper small.en inference with Rust feature extraction and tract kernels.
//! ONNX supplies unchanged int8/f32 weights; no foreign inference runtime.
mod embedding;
mod features;
mod graph;
mod model;
mod quantize;
mod scale;
use base64::{engine::general_purpose::STANDARD, Engine};
use embedding::Embedding;
use features::Features;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
};
use tract_linalg::multithread::Executor;
use tract_onnx::prelude::*;
use tract_onnx::tract_core::{
    internal::{ensure, format_err},
    runtime::RunOptions,
};
const STATE: usize = 768;
const LAYERS: usize = 12;
const VOCAB: usize = 51864;
pub struct Recognizer {
    encoder: Arc<TypedRunnableModel>,
    blocks: Vec<Arc<TypedRunnableModel>>,
    norm: Arc<TypedRunnableModel>,
    embedding: Embedding,
    positions: Tensor,
    tokens: HashMap<usize, Vec<u8>>,
    pool: Arc<rayon::ThreadPool>,
    features: Features,
}
fn plan(model: TypedModel, pool: &Arc<rayon::ThreadPool>) -> TractResult<Arc<TypedRunnableModel>> {
    let model = graph::fold_weight_sums(model)?;
    let model = quantize::install(model)?;
    let model = scale::install(model)?;
    TypedRunnableModel::new_with_options(
        model,
        &RunOptions {
            executor: Some(Executor::MultiThread(pool.clone())),
            ..Default::default()
        },
    )
}
impl Recognizer {
    pub fn load(
        encoder: impl AsRef<Path>,
        decoder: impl AsRef<Path>,
        tokens: impl AsRef<Path>,
        threads: usize,
    ) -> TractResult<Self> {
        ensure!((1..=32).contains(&threads), "Expected 1–32 threads");
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|n| format!("whisper-{n}"))
                .build()?,
        );
        let encoder = encoder.as_ref().canonicalize()?;
        let decoder = decoder.as_ref().canonicalize()?;
        let encoder_dir = encoder
            .parent()
            .and_then(Path::to_str)
            .ok_or_else(|| format_err!("Expected UTF-8 model directory"))?;
        let decoder_dir = decoder
            .parent()
            .and_then(Path::to_str)
            .ok_or_else(|| format_err!("Expected UTF-8 model directory"))?;
        let loader = tract_onnx::onnx();
        // Compile the encoder before adding resident decoder weights.
        let (mut p, _) = model::load(&encoder, 128 * 1024 * 1024, false)?;
        let meta: HashMap<_, _> = p
            .metadata_props
            .iter()
            .map(|p| (p.key.as_str(), p.value.as_str()))
            .collect();
        for (key, val) in [
            ("model_type", "whisper-small.en"),
            ("version", "1"),
            ("n_mels", "80"),
            ("n_audio_state", "768"),
            ("n_audio_head", "12"),
            ("n_audio_layer", "12"),
            ("n_text_layer", "12"),
            ("n_text_state", "768"),
            ("n_text_ctx", "448"),
            ("n_vocab", "51864"),
            ("sot_sequence", "50257"),
            ("eot", "50256"),
            ("no_timestamps", "50362"),
            ("is_multilingual", "0"),
        ] {
            ensure!(
                meta.get(key).copied() == Some(val),
                "Unsupported Whisper metadata {key}"
            );
        }
        ensure!(
            p.graph.as_ref().is_some_and(|g| g.input.len() == 1
                && g.input[0].name == "mel"
                && g.output
                    .iter()
                    .map(|v| v.name.as_str())
                    .eq(["n_layer_cross_k", "n_layer_cross_v"])),
            "Unexpected encoder interface"
        );
        graph::check_export(p.graph.as_ref().unwrap(), 0x8739_0418_f7ca_a917)?;
        graph::split_encoder_outputs(&mut p)?;
        graph::clear_shapes(&mut p)?;
        let parsed = loader.parse(&p, Some(encoder_dir))?;
        ensure!(
            parsed.unresolved_inputs.is_empty(),
            "Unresolved encoder inputs"
        );
        let mut model = parsed.model;
        drop(p);
        let f = model.symbols.sym("F");
        model.set_input_fact(0, f32::fact([1.to_dim(), 80.to_dim(), f.to_dim()]).into())?;
        let encoder = plan(model.into_optimized()?, &pool)?;

        let (p, embedding) = model::load(&decoder, 280 * 1024 * 1024, true)?;
        let embedding = embedding.ok_or_else(|| format_err!("Missing embedding"))?;
        let g = p
            .graph
            .as_ref()
            .ok_or_else(|| format_err!("Missing decoder graph"))?;
        graph::check_export(g, 0xcd61_f134_021f_2a7b)?;
        ensure!(
            g.input.iter().map(|v| v.name.as_str()).eq([
                "tokens",
                "in_n_layer_self_k_cache",
                "in_n_layer_self_v_cache",
                "n_layer_cross_k",
                "n_layer_cross_v",
                "offset"
            ]),
            "Unexpected decoder interface"
        );
        let tensor = |name: &str| -> TractResult<Tensor> {
            let t = g
                .initializer
                .iter()
                .find(|t| t.name == name)
                .ok_or_else(|| format_err!("Missing {name}"))?;
            tract_onnx::tensor::load_tensor(&*loader.provider, t, Some(decoder_dir))
        };
        let positions = tensor("textDecoder.positional_embedding")?;
        ensure!(
            embedding.shape() == [VOCAB, STATE] && positions.shape() == [448, STATE],
            "Unexpected embedding shape"
        );
        let blocks = (0..LAYERS)
            .map(|i| plan(graph::Builder::new(g, decoder_dir).block(i)?, &pool))
            .collect::<TractResult<Vec<_>>>()?;
        let norm = plan(graph::Builder::new(g, decoder_dir).final_norm()?, &pool)?;
        drop(p);

        ensure!(
            std::fs::metadata(tokens.as_ref())?.len() <= 2 * 1024 * 1024,
            "Token table exceeds 2 MiB"
        );
        let mut table = HashMap::new();
        for line in std::fs::read_to_string(tokens)?.lines() {
            let (bytes, id) = line
                .split_once(' ')
                .ok_or_else(|| format_err!("Invalid token line"))?;
            let id: usize = id.parse()?;
            ensure!(
                id < VOCAB && table.insert(id, STANDARD.decode(bytes)?).is_none(),
                "Invalid token ID"
            );
        }
        ensure!(
            table.len() == 50256 && (0..50256).all(|i| table.contains_key(&i)),
            "Unexpected English token table"
        );
        Ok(Self {
            encoder,
            blocks,
            norm,
            embedding,
            positions,
            tokens: table,
            pool,
            features: Features::default(),
        })
    }
    pub fn features(&self, samples: &[f32]) -> TractResult<(Tensor, usize)> {
        self.features.compute(samples)
    }
    pub fn embedding_storage(&self) -> &'static str {
        self.embedding.storage()
    }
    pub fn embedding_bytes(&self) -> usize {
        self.embedding.bytes()
    }
    pub fn transcribe(&self, samples: &[f32]) -> TractResult<String> {
        self.recognize(samples, None)
    }
    /// Explicit offline numerical diagnostic. Creates a new directory; never
    /// writes audio/intermediates during ordinary recognition.
    pub fn diagnose(&self, samples: &[f32], directory: &Path) -> TractResult<String> {
        std::fs::create_dir(directory)?;
        self.recognize(samples, Some(directory))
    }
    fn recognize(&self, samples: &[f32], trace: Option<&Path>) -> TractResult<String> {
        let mut profile = BTreeMap::new();
        let (features, frames) = self.features.compute(samples)?;
        if let Some(dir) = trace {
            write_tensor(&dir.join("mel.f32"), &features)?;
        }
        let encoder_start = std::time::Instant::now();
        let cross = run(
            &self.encoder,
            tvec!(features.into_tvalue()),
            trace.map(|_| &mut profile),
            "encoder",
        )?;
        let encoder_seconds = encoder_start.elapsed().as_secs_f64();
        let decoder_start = std::time::Instant::now();
        ensure!(
            cross.len() == LAYERS * 2 && cross.iter().all(|t| t.rank() == 4),
            "Unexpected encoder outputs"
        );
        let time = cross[0].shape()[2];
        ensure!(
            cross.iter().all(|t| t.shape() == [1, 1, time, STATE]),
            "Unexpected cross-cache shape"
        );
        if let Some(dir) = trace {
            write_tensors(
                &dir.join("cross-k.f32"),
                cross[..LAYERS].iter().map(|v| &**v),
            )?;
            write_tensors(
                &dir.join("cross-v.f32"),
                cross[LAYERS..].iter().map(|v| &**v),
            )?;
        }
        let mut cross = cross
            .into_iter()
            .map(|t| Ok(t.into_tensor().into_shape(&[1, time, STATE])?.into_tvalue()));
        let keys = cross
            .by_ref()
            .take(LAYERS)
            .collect::<TractResult<Vec<_>>>()?;
        let values = cross.collect::<TractResult<Vec<_>>>()?;
        let mut k = (0..LAYERS)
            .map(|_| Ok(Tensor::zero::<f32>(&[1, 0, STATE])?.into_tvalue()))
            .collect::<TractResult<Vec<_>>>()?;
        let mut v = k.clone();
        let mut input = vec![50257, 50362];
        let mut offset = 0;
        let mut bytes = Vec::new();
        let mut predicted = Vec::new();
        let maximum = ((frames as f64 / 100.0 * 6.0) as usize).min(224);
        for step in 0..=maximum {
            let n = input.len();
            let mut x = Vec::with_capacity(n * STATE);
            let positions = self.positions.try_as_plain_ram()?.as_slice::<f32>()?;
            for (i, &token) in input.iter().enumerate() {
                self.embedding.append_positioned(
                    token,
                    &positions[(offset + i) * STATE..(offset + i + 1) * STATE],
                    &mut x,
                )?;
            }
            let mut x = Tensor::from_shape(&[1, n, STATE], &x)?.into_tvalue();
            let total = offset + n;
            let mask: Vec<f32> = (0..n)
                .flat_map(|i| {
                    (0..total).map(move |j| {
                        if n > 1 && j > i {
                            f32::NEG_INFINITY
                        } else {
                            0.0
                        }
                    })
                })
                .collect();
            let mask = Tensor::from_shape(&[1, 1, n, total], &mask)?.into_tvalue();
            for layer in 0..LAYERS {
                let out = run(
                    &self.blocks[layer],
                    tvec!(
                        x,
                        k[layer].clone(),
                        v[layer].clone(),
                        keys[layer].clone(),
                        values[layer].clone(),
                        mask.clone()
                    ),
                    trace.map(|_| &mut profile),
                    "decoder",
                )?;
                let mut out = out.into_iter();
                x = out.next().unwrap();
                k[layer] = out.next().unwrap();
                v[layer] = out.next().unwrap();
            }
            let last = Tensor::from_shape(
                &[1, 1, STATE],
                &x.try_as_plain_ram()?.as_slice::<f32>()?[(n - 1) * STATE..n * STATE],
            )?
            .into_tvalue();
            let norm = run(
                &self.norm,
                tvec!(last),
                trace.map(|_| &mut profile),
                "final_norm",
            )?;
            let x = norm[0].try_as_plain_ram()?.as_slice::<f32>()?;
            let logits = self.embedding.project(x, &self.pool)?;
            ensure!(
                logits.iter().all(|x| x.is_finite()),
                "Non-finite decoder logits"
            );
            let token = logits
                .iter()
                .enumerate()
                .fold(0, |best, (i, v)| if *v > logits[best] { i } else { best });
            if let Some(dir) = trace {
                write_tensor(
                    &dir.join(format!("step-{step:03}.f32")),
                    &Tensor::from_shape(&[VOCAB], &logits)?,
                )?;
                predicted.push(token);
            }
            if token == 50256 || step == maximum {
                break;
            }
            if let Some(text) = self.tokens.get(&token) {
                bytes.extend_from_slice(text);
            }
            offset += n;
            input.clear();
            input.push(token);
        }
        let text = String::from_utf8(bytes)?.trim().to_owned();
        if let Some(dir) = trace {
            std::fs::write(
                dir.join("trace.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"text":text,"tokens":predicted,"encoder_seconds":encoder_seconds,"decoder_seconds":decoder_start.elapsed().as_secs_f64(),"feature_frames":frames,"mel_shape":[1,80,frames+1000],"cross_shape":[12,1,time,768],"profile":profile}),
                )?,
            )?;
        }
        Ok(text)
    }
}
fn run(
    plan: &Arc<TypedRunnableModel>,
    inputs: TVec<TValue>,
    profile: Option<&mut BTreeMap<String, (usize, f64)>>,
    stage: &str,
) -> TractResult<TVec<TValue>> {
    let Some(profile) = profile else {
        return plan.run(inputs);
    };
    plan.spawn()?
        .run_plan_with_eval(inputs, |context, state, node, inputs| {
            let start = std::time::Instant::now();
            let result = tract_onnx::tract_core::plan::eval(context, state, node, inputs);
            let entry = profile
                .entry(format!("{stage}/{} [{}]", node.name, node.op.name()))
                .or_default();
            entry.0 += 1;
            entry.1 += start.elapsed().as_secs_f64();
            result
        })
}
fn write_tensor(path: &Path, tensor: &Tensor) -> TractResult<()> {
    write_tensors(path, std::iter::once(tensor))
}
fn write_tensors<'a>(path: &Path, tensors: impl Iterator<Item = &'a Tensor>) -> TractResult<()> {
    use std::io::Write;
    let mut out = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?,
    );
    for tensor in tensors {
        for value in tensor.try_as_plain_ram()?.as_slice::<f32>()? {
            out.write_all(&value.to_le_bytes())?;
        }
    }
    out.flush()?;
    Ok(())
}
