//! A made-up embedding model in Model2Vec's layout, for semantic search
//! through a real daemon: tests can't download the pinned one. Each word
//! has a hand-set vector on four axes (teeth, cars, food, anything else),
//! so which tasks are close to a query is known in advance.

use std::path::Path;

use serde_json::{Map, Value, json};

/// Each word the model knows and its vector; anything else is unknown.
const WORDS: &[(&str, [f32; 4])] = &[
    ("dentist", [1.0, 0.0, 0.0, 0.0]),
    ("teeth", [1.0, 0.0, 0.0, 0.0]),
    ("cleaning", [0.3, 0.0, 0.0, 0.7]),
    ("vehicle", [0.0, 1.0, 0.0, 0.0]),
    ("car", [0.0, 1.0, 0.0, 0.0]),
    ("insurance", [0.0, 0.5, 0.0, 0.5]),
    ("groceries", [0.0, 0.0, 1.0, 0.0]),
    ("milk", [0.0, 0.0, 1.0, 0.0]),
    ("book", [0.0, 0.0, 0.0, 1.0]),
    ("renew", [0.0, 0.0, 0.0, 1.0]),
    ("buy", [0.0, 0.0, 0.0, 1.0]),
];

/// Write the model's three files into `dir`.
pub fn write(dir: &Path) {
    std::fs::create_dir_all(dir).expect("model dir");
    // Row 0 is the unknown token, which Model2Vec drops before pooling.
    let mut vocab = Map::new();
    vocab.insert("[UNK]".into(), json!(0));
    let mut rows: Vec<f32> = vec![0.0; 4];
    for (index, (word, vector)) in WORDS.iter().enumerate() {
        vocab.insert((*word).into(), json!(index + 1));
        rows.extend_from_slice(vector);
    }
    let tokenizer = json!({
        "version": "1.0",
        "truncation": null,
        "padding": null,
        "added_tokens": [],
        "normalizer": { "type": "Lowercase" },
        "pre_tokenizer": { "type": "Whitespace" },
        "post_processor": null,
        "decoder": null,
        "model": { "type": "WordLevel", "vocab": Value::Object(vocab), "unk_token": "[UNK]" }
    });
    std::fs::write(dir.join("tokenizer.json"), tokenizer.to_string()).expect("tokenizer");
    std::fs::write(
        dir.join("config.json"),
        json!({ "normalize": true }).to_string(),
    )
    .expect("config");
    std::fs::write(
        dir.join("model.safetensors"),
        safetensors(&rows, WORDS.len() + 1),
    )
    .expect("model");
}

/// One F32 tensor, `embeddings`, of `count` rows of 4, in the safetensors
/// format: a little-endian header length, the JSON header, the data.
fn safetensors(rows: &[f32], count: usize) -> Vec<u8> {
    let data: Vec<u8> = rows.iter().flat_map(|value| value.to_le_bytes()).collect();
    let mut header = json!({
        "embeddings": { "dtype": "F32", "shape": [count, 4], "data_offsets": [0, data.len()] }
    })
    .to_string();
    // The format pads the header to a multiple of 8 with spaces.
    while !header.len().is_multiple_of(8) {
        header.push(' ');
    }
    let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(&data);
    bytes
}
