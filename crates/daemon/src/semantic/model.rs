//! The embedding model (S19, D-062): Model2Vec's potion-base-8M, static
//! embeddings run by `model2vec-rs`, pure Rust. It isn't in the binary:
//! the daemon downloads its three files from Hugging Face at a pinned
//! revision the first time semantic search is on, checks each against a
//! pinned SHA-256, and keeps them in the instance's data directory. Every
//! load checks them again, so a damaged or swapped file is downloaded
//! afresh rather than used.

use std::path::{Path, PathBuf};
use std::time::Duration;

use model2vec_rs::model::StaticModel;
use ms_todo_graph::private_file::atomic_write_mode_0600;

use crate::outbox::move_job::spool::sha256_hex;

/// The model on Hugging Face, and the commit its files are pinned to.
const REPO: &str = "minishlab/potion-base-8M";
const REVISION: &str = "bf8b056651a2c21b8d2565580b8569da283cab23";
const BASE_URL: &str = "https://huggingface.co";

/// What vectors are tagged with in the store: a new model or revision
/// re-embeds every task.
pub(crate) const MODEL_ID: &str = "potion-base-8M@bf8b056";

/// A file of the model, as pinned.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModelFile {
    pub name: &'static str,
    /// SHA-256, lowercase hex.
    pub sha256: &'static str,
    pub bytes: u64,
}

/// The pinned files (S19). `config.json` says whether vectors are
/// normalised; the tokenizer and the embedding table are the model.
pub(crate) const FILES: [ModelFile; 3] = [
    ModelFile {
        name: "config.json",
        sha256: "2a6ac0e9aaa356a68a5688070db78fc3a464fefe85d2f06a1905ce3718687553",
        bytes: 202,
    },
    ModelFile {
        name: "tokenizer.json",
        sha256: "e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50",
        bytes: 683_666,
    },
    ModelFile {
        name: "model.safetensors",
        sha256: "f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2",
        bytes: 30_236_760,
    },
];

/// How many bytes a first download fetches.
pub(crate) fn download_bytes() -> u64 {
    FILES.iter().map(|file| file.bytes).sum()
}

/// The model's directory under the instance's data directory, named for
/// the revision so another one never reads these files.
pub(crate) fn pinned_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join(MODEL_ID.replace('@', "-"))
}

/// Where the model's files come from.
#[derive(Clone, Debug)]
pub(crate) enum Source {
    /// The pinned files, downloaded into `dir` when missing or damaged.
    Pinned { dir: PathBuf },
    /// A model already in `dir`, used as it is: only debug builds, for
    /// tests (`MS_TODO_SEMANTIC_MODEL_DIR`), which can't download.
    Local { dir: PathBuf },
}

impl Source {
    pub fn dir(&self) -> &Path {
        match self {
            Self::Pinned { dir } | Self::Local { dir } => dir,
        }
    }
}

/// The loaded model, making unit-length vectors.
pub(crate) struct Embedder {
    model: StaticModel,
}

impl Embedder {
    /// One vector per text, unit length (or all zeros for a text with no
    /// word the model knows). CPU-bound: call it off the async threads
    /// for more than a query's worth.
    pub fn embed(&self, texts: &[String]) -> Vec<Vec<f32>> {
        // 512 tokens is the model's own default cap per text.
        let mut vectors = self.model.encode_with_args(texts, Some(512), 256);
        for vector in &mut vectors {
            normalise(vector);
        }
        vectors
    }

    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        self.embed(&[text.to_owned()])
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

fn normalise(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

/// Load the model, downloading any pinned file that's missing or doesn't
/// match its checksum.
pub(crate) async fn load(source: &Source) -> Result<Embedder, String> {
    let mut files = Vec::with_capacity(FILES.len());
    match source {
        Source::Local { dir } => {
            for file in FILES {
                let path = dir.join(file.name);
                files.push(
                    std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
                );
            }
        }
        Source::Pinned { dir } => {
            let client = http_client()?;
            for file in FILES {
                let url = format!("{BASE_URL}/{REPO}/resolve/{REVISION}/{}", file.name);
                files.push(ensure_file(&client, &url, &file, dir).await?);
            }
        }
    }
    let [config, tokenizer, model]: [Vec<u8>; 3] = files
        .try_into()
        .map_err(|_| "the model has three files".to_owned())?;
    // Parsing the tokenizer and the 30 MB table takes tens of milliseconds.
    tokio::task::spawn_blocking(move || {
        StaticModel::from_bytes(tokenizer, model, config, None)
            .map(|model| Embedder { model })
            .map_err(|error| format!("the embedding model can't be read: {error:#}"))
    })
    .await
    .map_err(|error| format!("loading the embedding model failed: {error}"))?
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("ms-todo/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        // Per read, not for the whole 30 MB: a slow line still finishes.
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("can't make an HTTP client: {error}"))
}

/// `file`'s bytes from `dir`, or from `url` when it's missing there or
/// doesn't match its checksum. A download is checked before it's kept,
/// and written atomically (0600, in a 0700 directory), so a failed one
/// leaves nothing that looks whole.
pub(crate) async fn ensure_file(
    client: &reqwest::Client,
    url: &str,
    file: &ModelFile,
    dir: &Path,
) -> Result<Vec<u8>, String> {
    let path = dir.join(file.name);
    match tokio::fs::read(&path).await {
        Ok(bytes) if sha256_hex(&bytes) == file.sha256 => return Ok(bytes),
        Ok(_) => eprintln!(
            "ms-todo daemon: {} doesn't match its checksum; downloading it again",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    eprintln!(
        "ms-todo daemon: downloading the embedding model's {} ({} bytes) from {url}",
        file.name, file.bytes
    );
    let bytes = download(client, url, file).await?;
    let found = sha256_hex(&bytes);
    if found != file.sha256 {
        return Err(format!(
            "the downloaded {} has SHA-256 {found}, not the pinned {}; not using it",
            file.name, file.sha256
        ));
    }
    tokio::task::spawn_blocking(move || {
        atomic_write_mode_0600(&path, &bytes)
            .map(|()| bytes)
            .map_err(|error| format!("{}: {error}", path.display()))
    })
    .await
    .map_err(|error| format!("saving the embedding model failed: {error}"))?
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    file: &ModelFile,
) -> Result<Vec<u8>, String> {
    let failed = |error: reqwest::Error| {
        format!(
            "downloading the embedding model's {} failed: {}",
            file.name,
            ms_todo_core::message_with_causes(&error)
        )
    };
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?;
    let mut bytes = Vec::with_capacity(usize::try_from(file.bytes).unwrap_or(0));
    while let Some(chunk) = response.chunk().await.map_err(failed)? {
        bytes.extend_from_slice(&chunk);
        // A server sending more than the pinned size can't be sending
        // the pinned file; stop before holding all of it.
        if bytes.len() as u64 > file.bytes {
            return Err(format!(
                "the embedding model's {} is larger than the pinned {} bytes; not using it",
                file.name, file.bytes
            ));
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const BODY: &[u8] = b"pinned bytes";

    fn pinned() -> ModelFile {
        ModelFile {
            name: "model.bin",
            // SHA-256 of BODY.
            sha256: "977d59924a4dfd9d4e93c74467f3253786327c8c28e69a75f30cf0429dab77c7",
            bytes: BODY.len() as u64,
        }
    }

    async fn serving(body: &[u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_download_is_checked_then_kept_and_read_from_disk_after() {
        let server = serving(BODY).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let client = http_client().expect("client");
        let url = format!("{}/model.bin", server.uri());
        let file = pinned();
        assert_eq!(
            ensure_file(&client, &url, &file, dir.path()).await,
            Ok(BODY.to_vec())
        );
        assert_eq!(
            std::fs::read(dir.path().join("model.bin")).expect("kept"),
            BODY
        );
        assert!(!dir.path().join("model.bin.part").exists());
        // From disk now: the server isn't asked again.
        server.reset().await;
        assert_eq!(
            ensure_file(&client, &url, &file, dir.path()).await,
            Ok(BODY.to_vec())
        );
    }

    #[tokio::test]
    async fn a_file_that_fails_its_checksum_is_never_kept() {
        let server = serving(b"other bytes!").await;
        let dir = tempfile::tempdir().expect("tempdir");
        let client = http_client().expect("client");
        let url = format!("{}/model.bin", server.uri());
        let error = ensure_file(&client, &url, &pinned(), dir.path())
            .await
            .expect_err("a mismatch");
        assert!(error.contains("not the pinned"), "{error}");
        assert!(!dir.path().join("model.bin").exists());

        // A damaged file on disk is replaced by a good download.
        let server = serving(BODY).await;
        std::fs::write(dir.path().join("model.bin"), b"damaged").expect("write");
        let url = format!("{}/model.bin", server.uri());
        assert_eq!(
            ensure_file(&client, &url, &pinned(), dir.path()).await,
            Ok(BODY.to_vec())
        );
    }

    #[tokio::test]
    async fn a_download_larger_than_pinned_stops() {
        let server = serving(b"pinned bytes and then some").await;
        let dir = tempfile::tempdir().expect("tempdir");
        let client = http_client().expect("client");
        let url = format!("{}/model.bin", server.uri());
        let error = ensure_file(&client, &url, &pinned(), dir.path())
            .await
            .expect_err("too large");
        assert!(error.contains("larger than"), "{error}");
    }

    #[test]
    fn the_pinned_files_add_up_to_about_31_mb() {
        assert_eq!(download_bytes(), 30_920_628);
        assert!(FILES.iter().all(|file| file.sha256.len() == 64));
        assert_eq!(sha256_hex(BODY), pinned().sha256);
    }
}
