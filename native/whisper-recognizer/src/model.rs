//! Bounded ONNX import. prost still decodes the schema; only the outer model,
//! graph and initializer envelopes are streamed. The large embedding payload
//! goes straight from the file to its lossless compact representation. No full
//! serialized embedding Vec or expanded f32 embedding is retained. Other raw
//! payloads become bounded ranges in this same file, using ONNX's external-data
//! interface internally; tract reads only the weights for the current graph.
use crate::embedding::Embedding;
use prost::Message;
use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};
use tract_onnx::tract_core::internal::{ensure, format_err};
use tract_onnx::{pb, prelude::*};

const EMBEDDING: &str = "textDecoder.token_embedding.weight";
const MAX_FIELD: u64 = 16 * 1024 * 1024;

struct Field {
    tag: u32,
    wire: u8,
    prefix: Vec<u8>,
    start: u64,
    end: u64,
}

fn varint(reader: &mut (impl Read + Seek), end: u64, bytes: &mut Vec<u8>) -> TractResult<u64> {
    let mut value = 0_u64;
    for i in 0..10 {
        ensure!(reader.stream_position()? < end, "Truncated ONNX field");
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        let byte = byte[0];
        bytes.push(byte);
        ensure!(i != 9 || byte <= 1, "Overflowing ONNX varint");
        value |= u64::from(byte & 127) << (i * 7);
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(format_err!("Overflowing ONNX varint"))
}

fn field(reader: &mut (impl Read + Seek), end: u64) -> TractResult<Field> {
    let mut prefix = Vec::with_capacity(20);
    let key = varint(reader, end, &mut prefix)?;
    let tag = key >> 3;
    ensure!((1..=0x1fff_ffff).contains(&tag), "Invalid ONNX field tag");
    let wire = (key & 7) as u8;
    let length = match wire {
        0 => {
            varint(reader, end, &mut prefix)?;
            0
        }
        1 => 8,
        2 => varint(reader, end, &mut prefix)?,
        5 => 4,
        _ => return Err(format_err!("Unsupported ONNX wire type")),
    };
    let start = reader.stream_position()?;
    let stop = start
        .checked_add(length)
        .ok_or_else(|| format_err!("ONNX field overflow"))?;
    ensure!(stop <= end, "ONNX field exceeds its message");
    Ok(Field {
        tag: tag as u32,
        wire,
        prefix,
        start,
        end: stop,
    })
}

fn merge<M: Message>(reader: &mut impl Read, mut field: Field, message: &mut M) -> TractResult<()> {
    let length = field.end - field.start;
    ensure!(length <= MAX_FIELD, "ONNX metadata field exceeds 16 MiB");
    let start = field.prefix.len();
    field.prefix.resize(start + usize::try_from(length)?, 0);
    reader.read_exact(&mut field.prefix[start..])?;
    message.merge(field.prefix.as_slice())?;
    Ok(())
}

fn tensor(
    reader: &mut (impl Read + Seek),
    end: u64,
    compact: bool,
    embedding: &mut Option<Embedding>,
    external: Option<&str>,
) -> TractResult<Option<pb::TensorProto>> {
    let mut tensor = pb::TensorProto::default();
    let mut raw = None;
    while reader.stream_position()? < end {
        let f = field(reader, end)?;
        if f.tag == 9 {
            ensure!(
                f.wire == 2 && raw.is_none(),
                "Invalid or duplicate ONNX raw_data"
            );
            reader.seek(SeekFrom::Start(f.end))?;
            raw = Some(f);
        } else {
            merge(reader, f, &mut tensor)?;
        }
    }
    ensure!(
        raw.as_ref().is_some_and(|r| r.end > r.start)
            || (tensor.external_data.is_empty()
                && tensor.data_location != Some(pb::tensor_proto::DataLocation::External as i32)),
        "Expected self-contained ONNX weights"
    );
    if compact && tensor.name == EMBEDDING {
        ensure!(
            embedding.is_none() && tensor.data_type == 1,
            "Invalid embedding initializer"
        );
        if let Some(raw) = raw {
            reader.seek(SeekFrom::Start(raw.start))?;
            *embedding = Some(Embedding::read_raw(
                reader,
                &tensor.dims,
                raw.end - raw.start,
            )?);
        } else {
            let loader = tract_onnx::onnx();
            *embedding = Some(Embedding::new(tract_onnx::tensor::load_tensor(
                &*loader.provider,
                &tensor,
                None,
            )?)?);
        }
        reader.seek(SeekFrom::Start(end))?;
        return Ok(None);
    }
    if let Some(raw) = raw {
        let length = raw.end - raw.start;
        ensure!(length <= MAX_FIELD, "ONNX weight exceeds 16 MiB");
        if let Some(location) = external.filter(|_| length != 0) {
            tensor.data_location = Some(pb::tensor_proto::DataLocation::External as i32);
            tensor.external_data = [
                ("location", location.to_owned()),
                ("offset", raw.start.to_string()),
                ("length", length.to_string()),
            ]
            .into_iter()
            .map(|(key, value)| pb::StringStringEntryProto {
                key: key.into(),
                value,
            })
            .collect();
        } else {
            tensor.raw_data.resize(usize::try_from(length)?, 0);
            reader.seek(SeekFrom::Start(raw.start))?;
            reader.read_exact(&mut tensor.raw_data)?;
            reader.seek(SeekFrom::Start(end))?;
        }
    }
    Ok(Some(tensor))
}

fn graph(
    reader: &mut (impl Read + Seek),
    end: u64,
    compact: bool,
    embedding: &mut Option<Embedding>,
    external: Option<&str>,
) -> TractResult<pb::GraphProto> {
    let mut graph = pb::GraphProto::default();
    let mut names = HashSet::new();
    while reader.stream_position()? < end {
        let f = field(reader, end)?;
        if f.tag == 5 {
            ensure!(f.wire == 2, "Invalid ONNX initializer wire type");
            if let Some(t) = tensor(reader, f.end, compact, embedding, external)? {
                ensure!(names.insert(t.name.clone()), "Duplicate ONNX initializer");
                graph.initializer.push(t);
            }
        } else {
            merge(reader, f, &mut graph)?;
        }
    }
    Ok(graph)
}

fn read_model(
    reader: &mut (impl Read + Seek),
    end: u64,
    compact: bool,
    external: Option<&str>,
) -> TractResult<(pb::ModelProto, Option<Embedding>)> {
    let mut model = pb::ModelProto::default();
    let mut embedding = None;
    while reader.stream_position()? < end {
        let f = field(reader, end)?;
        if f.tag == 7 {
            ensure!(
                f.wire == 2 && model.graph.is_none(),
                "Invalid or duplicate ONNX graph"
            );
            model.graph = Some(graph(reader, f.end, compact, &mut embedding, external)?);
        } else {
            merge(reader, f, &mut model)?;
        }
    }
    ensure!(model.graph.is_some(), "Missing ONNX graph");
    ensure!(
        !compact || embedding.is_some(),
        "Missing embedding initializer"
    );
    Ok((model, embedding))
}

pub fn load(
    path: &Path,
    limit: u64,
    compact: bool,
) -> TractResult<(pb::ModelProto, Option<Embedding>)> {
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    ensure!(length <= limit, "Whisper model exceeds size limit");
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format_err!("Expected a UTF-8 model filename"))?;
    read_model(&mut BufReader::new(file), length, compact, Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture() -> pb::ModelProto {
        pb::ModelProto {
            ir_version: 8,
            graph: Some(pb::GraphProto {
                name: "streamed".into(),
                initializer: vec![
                    pb::TensorProto {
                        name: EMBEDDING.into(),
                        data_type: 1,
                        dims: vec![2, 8],
                        raw_data: [0.0_f32, -0.0, 1.0, -1.0]
                            .into_iter()
                            .cycle()
                            .take(16)
                            .flat_map(f32::to_le_bytes)
                            .collect(),
                        ..Default::default()
                    },
                    pb::TensorProto {
                        name: "weight".into(),
                        data_type: 3,
                        dims: vec![4],
                        raw_data: vec![1, 255, 3, 128],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn streamed_schema_and_weights_match_prost() -> TractResult<()> {
        let original = fixture();
        let bytes = original.encode_to_vec();
        let (actual, embedding) =
            read_model(&mut Cursor::new(&bytes), bytes.len() as u64, false, None)?;
        assert_eq!(actual, original);
        assert!(embedding.is_none());
        let (actual, embedding) =
            read_model(&mut Cursor::new(&bytes), bytes.len() as u64, true, None)?;
        let mut expected = original;
        expected.graph.as_mut().unwrap().initializer.remove(0);
        assert_eq!(actual, expected);
        let embedding = embedding.unwrap();
        assert_eq!(embedding.bytes(), 32);
        let mut output = vec![];
        embedding.append_positioned(1, &[-0.0; 8], &mut output)?;
        assert_eq!(
            output.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            [0.0_f32, -0.0, 1.0, -1.0]
                .into_iter()
                .cycle()
                .take(8)
                .map(|v| (v + -0.0).to_bits())
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn lazy_weight_ranges_resolve_to_the_original_payloads() -> TractResult<()> {
        let original = fixture();
        let bytes = original.encode_to_vec();
        let (actual, embedding) = read_model(
            &mut Cursor::new(&bytes),
            bytes.len() as u64,
            false,
            Some("model.onnx"),
        )?;
        assert!(embedding.is_none());
        for (expected, actual) in original
            .graph
            .unwrap()
            .initializer
            .iter()
            .zip(&actual.graph.unwrap().initializer)
        {
            assert!(actual.raw_data.is_empty());
            assert_eq!(
                actual.data_location,
                Some(pb::tensor_proto::DataLocation::External as i32)
            );
            let fields: std::collections::HashMap<_, _> = actual
                .external_data
                .iter()
                .map(|p| (p.key.as_str(), p.value.as_str()))
                .collect();
            assert_eq!(fields["location"], "model.onnx");
            let start: usize = fields["offset"].parse()?;
            let length: usize = fields["length"].parse()?;
            assert_eq!(&bytes[start..start + length], expected.raw_data);
        }
        let mut bad = fixture();
        let t = &mut bad.graph.as_mut().unwrap().initializer[1];
        t.raw_data.clear();
        t.data_location = Some(pb::tensor_proto::DataLocation::External as i32);
        let bytes = bad.encode_to_vec();
        assert!(read_model(
            &mut Cursor::new(&bytes),
            bytes.len() as u64,
            false,
            Some("model.onnx")
        )
        .is_err());
        // An explicitly encoded empty raw_data field must not enable paths
        // supplied by the model. Only nonempty ranges in this file are used.
        let tensor = [
            envelope(9, &[]),
            bad.graph.unwrap().initializer[1].encode_to_vec(),
        ]
        .concat();
        let bytes = envelope(7, &envelope(5, &tensor));
        assert!(read_model(
            &mut Cursor::new(&bytes),
            bytes.len() as u64,
            false,
            Some("model.onnx")
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn rejects_truncated_envelopes_overflows_and_duplicate_graphs() {
        let bytes = fixture().encode_to_vec();
        for cut in 0..bytes.len() {
            assert!(
                read_model(&mut Cursor::new(&bytes[..cut]), cut as u64, true, None).is_err(),
                "cut {cut}"
            );
        }
        for malformed in [
            vec![0],
            vec![0x3b],
            vec![0x3a, 127],
            vec![
                0x3a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2,
            ],
        ] {
            assert!(read_model(
                &mut Cursor::new(&malformed),
                malformed.len() as u64,
                false,
                None
            )
            .is_err());
        }
        let duplicated = [bytes.as_slice(), bytes.as_slice()].concat();
        assert!(read_model(
            &mut Cursor::new(&duplicated),
            duplicated.len() as u64,
            true,
            None
        )
        .is_err());
    }

    fn envelope(tag: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        prost::encoding::encode_key(tag, prost::encoding::WireType::LengthDelimited, &mut bytes);
        prost::encoding::encode_varint(payload.len() as u64, &mut bytes);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn reads_payload_before_name_and_rejects_duplicate_raw_data() -> TractResult<()> {
        let mut t = fixture().graph.unwrap().initializer.remove(0);
        let raw = envelope(9, &std::mem::take(&mut t.raw_data));
        let tensor = [raw.clone(), t.encode_to_vec()].concat();
        let bytes = envelope(7, &envelope(5, &tensor));
        let (_, embedding) = read_model(&mut Cursor::new(&bytes), bytes.len() as u64, true, None)?;
        assert_eq!(embedding.unwrap().shape(), [2, 8]);
        let tensor = [raw.clone(), raw, t.encode_to_vec()].concat();
        let bytes = envelope(7, &envelope(5, &tensor));
        assert!(read_model(&mut Cursor::new(&bytes), bytes.len() as u64, true, None).is_err());
        Ok(())
    }
}
