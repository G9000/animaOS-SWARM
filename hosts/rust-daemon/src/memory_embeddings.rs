use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anima_memory::{
    InMemoryVectorIndex, Memory, MemoryTextEmbedder, MemoryVectorError, MemoryVectorIndex,
    VectorMemoryHit,
};
use reqwest::blocking::Client as BlockingHttpClient;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock as AsyncRwLock;
use tracing::warn;

pub(crate) type SharedMemoryEmbeddings = Arc<AsyncRwLock<MemoryEmbeddingRuntime>>;

const DEFAULT_EMBEDDING_DIMENSION: usize = 96;
const MIN_EMBEDDING_DIMENSION: usize = 24;
const DEFAULT_OPENAI_EMBEDDING_DIMENSION: usize = 1536;
const DEFAULT_OLLAMA_EMBEDDING_DIMENSION: usize = 768;
const DEFAULT_EMBEDDING_TIMEOUT_MS: u64 = 15_000;
const LOCAL_PROVIDER: &str = "local";
const LOCAL_MODEL: &str = "local-semantic-v1";
const OPENAI_PROVIDER: &str = "openai";
const OPENAI_COMPATIBLE_PROVIDER: &str = "openai-compatible";
const OLLAMA_PROVIDER: &str = "ollama";
const DEFAULT_OPENAI_EMBEDDING_MODEL: &str = "text-embedding-3-small";
const DEFAULT_OLLAMA_EMBEDDING_MODEL: &str = "nomic-embed-text";
const DEFAULT_OPENAI_EMBEDDING_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_OLLAMA_EMBEDDING_BASE_URL: &str = "http://127.0.0.1:11434/v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MemoryEmbeddingStatus {
    pub(crate) enabled: bool,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) dimension: usize,
    pub(crate) vector_count: usize,
    pub(crate) persisted: bool,
    pub(crate) storage_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemoryEmbeddingRebuildReport {
    pub(crate) loaded_vectors: usize,
    pub(crate) rebuilt_vectors: usize,
    pub(crate) removed_stale_vectors: usize,
}

pub(crate) struct MemoryEmbeddingRuntime {
    enabled: bool,
    embedder: MemoryEmbeddingProvider,
    index: InMemoryVectorIndex<MemoryEmbeddingProvider>,
    store: Option<SqliteMemoryEmbeddingStore>,
}

impl MemoryEmbeddingRuntime {
    pub(crate) fn local_default() -> Self {
        Self::local(DEFAULT_EMBEDDING_DIMENSION)
    }

    pub(crate) fn local(dimension: usize) -> Self {
        let embedder = MemoryEmbeddingProvider::Local(LocalMemoryEmbedder::new(
            dimension.max(MIN_EMBEDDING_DIMENSION),
        ));
        Self {
            enabled: true,
            embedder: embedder.clone(),
            index: InMemoryVectorIndex::new(embedder),
            store: None,
        }
    }

    pub(crate) fn disabled() -> Self {
        let embedder =
            MemoryEmbeddingProvider::Local(LocalMemoryEmbedder::new(DEFAULT_EMBEDDING_DIMENSION));
        Self {
            enabled: false,
            embedder: embedder.clone(),
            index: InMemoryVectorIndex::new(embedder),
            store: None,
        }
    }

    pub(crate) fn from_env(default_sqlite_path: Option<PathBuf>) -> io::Result<Self> {
        let mode = std::env::var("ANIMAOS_RS_MEMORY_EMBEDDINGS")
            .unwrap_or_else(|_| LOCAL_PROVIDER.to_string())
            .to_ascii_lowercase();
        if matches!(mode.as_str(), "0" | "off" | "false" | "disabled") {
            return Ok(Self::disabled());
        }

        let dimension = embedding_dimension_from_env()?;
        let embedder = match mode.as_str() {
            LOCAL_PROVIDER | "local-hash" | "local-semantic" => {
                let dimension = dimension.unwrap_or(DEFAULT_EMBEDDING_DIMENSION);
                if dimension < MIN_EMBEDDING_DIMENSION {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS must be at least {MIN_EMBEDDING_DIMENSION} for local embeddings"
                        ),
                    ));
                }
                MemoryEmbeddingProvider::Local(LocalMemoryEmbedder::new(dimension))
            }
            OLLAMA_PROVIDER => MemoryEmbeddingProvider::OpenAiCompatible(
                OpenAiCompatibleMemoryEmbedder::from_env(OpenAiCompatibleEmbeddingConfig {
                    provider: OLLAMA_PROVIDER,
                    default_base_url: DEFAULT_OLLAMA_EMBEDDING_BASE_URL,
                    default_model: DEFAULT_OLLAMA_EMBEDDING_MODEL,
                    default_dimension: DEFAULT_OLLAMA_EMBEDDING_DIMENSION,
                    api_key_envs: &["ANIMAOS_RS_MEMORY_EMBEDDINGS_API_KEY", "OLLAMA_API_KEY"],
                    base_url_envs: &["ANIMAOS_RS_MEMORY_EMBEDDINGS_BASE_URL", "OLLAMA_BASE_URL"],
                    requires_key: false,
                    dimension,
                })?,
            ),
            OPENAI_PROVIDER => MemoryEmbeddingProvider::OpenAiCompatible(
                OpenAiCompatibleMemoryEmbedder::from_env(OpenAiCompatibleEmbeddingConfig {
                    provider: OPENAI_PROVIDER,
                    default_base_url: DEFAULT_OPENAI_EMBEDDING_BASE_URL,
                    default_model: DEFAULT_OPENAI_EMBEDDING_MODEL,
                    default_dimension: DEFAULT_OPENAI_EMBEDDING_DIMENSION,
                    api_key_envs: &[
                        "ANIMAOS_RS_MEMORY_EMBEDDINGS_API_KEY",
                        "OPENAI_API_KEY",
                        "OPENAI_KEY",
                        "OPENAI_TOKEN",
                    ],
                    base_url_envs: &["ANIMAOS_RS_MEMORY_EMBEDDINGS_BASE_URL", "OPENAI_BASE_URL"],
                    requires_key: true,
                    dimension,
                })?,
            ),
            OPENAI_COMPATIBLE_PROVIDER | "openai_compatible" | "compatible" => {
                MemoryEmbeddingProvider::OpenAiCompatible(
                    OpenAiCompatibleMemoryEmbedder::from_env(OpenAiCompatibleEmbeddingConfig {
                        provider: OPENAI_COMPATIBLE_PROVIDER,
                        default_base_url: DEFAULT_OPENAI_EMBEDDING_BASE_URL,
                        default_model: DEFAULT_OPENAI_EMBEDDING_MODEL,
                        default_dimension: DEFAULT_OPENAI_EMBEDDING_DIMENSION,
                        api_key_envs: &[
                            "ANIMAOS_RS_MEMORY_EMBEDDINGS_API_KEY",
                            "OPENAI_API_KEY",
                            "OPENAI_KEY",
                            "OPENAI_TOKEN",
                        ],
                        base_url_envs: &[
                            "ANIMAOS_RS_MEMORY_EMBEDDINGS_BASE_URL",
                            "OPENAI_BASE_URL",
                        ],
                        requires_key: false,
                        dimension,
                    })?,
                )
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "ANIMAOS_RS_MEMORY_EMBEDDINGS must be local, ollama, openai, openai-compatible, or disabled",
                ))
            }
        };

        let mut runtime = Self::from_provider(embedder);
        let storage_path =
            env_path("ANIMAOS_RS_MEMORY_EMBEDDINGS_SQLITE_FILE")?.or(default_sqlite_path);
        if let Some(path) = storage_path {
            runtime.store = Some(SqliteMemoryEmbeddingStore::new(path)?);
        }
        Ok(runtime)
    }

    fn from_provider(embedder: MemoryEmbeddingProvider) -> Self {
        Self {
            enabled: true,
            embedder: embedder.clone(),
            index: InMemoryVectorIndex::new(embedder),
            store: None,
        }
    }

    pub(crate) fn status(&self) -> MemoryEmbeddingStatus {
        MemoryEmbeddingStatus {
            enabled: self.enabled,
            provider: if self.enabled {
                self.embedder.provider().to_string()
            } else {
                "disabled".to_string()
            },
            model: if self.enabled {
                self.embedder.model().to_string()
            } else {
                "none".to_string()
            },
            dimension: self
                .index
                .dimension()
                .or_else(|| self.embedder.dimension_hint())
                .unwrap_or_default(),
            vector_count: self.index.len(),
            persisted: self.store.is_some(),
            storage_file: self.store.as_ref().map(|store| store.path.clone()),
        }
    }

    pub(crate) fn rebuild_from_memories(
        &mut self,
        memories: &[Memory],
    ) -> Result<MemoryEmbeddingRebuildReport, String> {
        let mut report = MemoryEmbeddingRebuildReport::default();
        self.index.clear();
        if !self.enabled {
            return Ok(report);
        }

        let valid_ids: HashSet<_> = memories.iter().map(|memory| memory.id.clone()).collect();
        let mut stored_by_id = HashMap::new();
        let storage_model = self.embedder.storage_model();
        if let Some(store) = &self.store {
            for stored in store.load_all().map_err(|error| error.to_string())? {
                if valid_ids.contains(&stored.memory_id)
                    && stored.model == storage_model
                    && self.embedder.accepts_dimension(stored.vector.len())
                {
                    stored_by_id.insert(stored.memory_id, stored.vector);
                }
            }
            report.removed_stale_vectors = store
                .delete_stale(&valid_ids)
                .map_err(|error| error.to_string())?;
        }

        for memory in memories {
            if let Some(vector) = stored_by_id.remove(&memory.id) {
                if self
                    .index
                    .upsert_embedding(memory.id.clone(), vector)
                    .is_ok()
                {
                    report.loaded_vectors += 1;
                    continue;
                }
            }

            let vector = self
                .embedder
                .embed(&memory.content)
                .map_err(|error| error.message().to_string())?;
            self.index
                .upsert_embedding(memory.id.clone(), vector.clone())
                .map_err(|error| error.message().to_string())?;
            if let Some(store) = &self.store {
                store
                    .upsert(&memory.id, &storage_model, &vector)
                    .map_err(|error| error.to_string())?;
            }
            report.rebuilt_vectors += 1;
        }

        Ok(report)
    }

    pub(crate) fn upsert_memory(&mut self, memory: &Memory) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let vector = self
            .embedder
            .embed(&memory.content)
            .map_err(|error| error.message().to_string())?;
        self.index
            .upsert_embedding(memory.id.clone(), vector.clone())
            .map_err(|error| error.message().to_string())?;
        if let Some(store) = &self.store {
            store
                .upsert(&memory.id, &self.embedder.storage_model(), &vector)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn remove_memories(&mut self, memory_ids: &[String]) -> Result<(), String> {
        for memory_id in memory_ids {
            self.index.remove(memory_id);
        }
        if let Some(store) = &self.store {
            store
                .delete_many(memory_ids)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

impl MemoryVectorIndex for MemoryEmbeddingRuntime {
    fn search(&self, query: &str, limit: usize) -> Vec<VectorMemoryHit> {
        if !self.enabled {
            return Vec::new();
        }
        self.index.search(query, limit)
    }
}

#[derive(Clone, Debug)]
enum MemoryEmbeddingProvider {
    Local(LocalMemoryEmbedder),
    OpenAiCompatible(OpenAiCompatibleMemoryEmbedder),
}

impl MemoryEmbeddingProvider {
    fn provider(&self) -> &str {
        match self {
            Self::Local(_) => LOCAL_PROVIDER,
            Self::OpenAiCompatible(embedder) => &embedder.provider,
        }
    }

    fn model(&self) -> &str {
        match self {
            Self::Local(_) => LOCAL_MODEL,
            Self::OpenAiCompatible(embedder) => &embedder.model,
        }
    }

    fn storage_model(&self) -> String {
        match self {
            Self::Local(_) => LOCAL_MODEL.to_string(),
            Self::OpenAiCompatible(embedder) => format!("{}:{}", embedder.provider, embedder.model),
        }
    }

    fn dimension_hint(&self) -> Option<usize> {
        match self {
            Self::Local(embedder) => Some(embedder.dimension),
            Self::OpenAiCompatible(embedder) => Some(embedder.dimension),
        }
    }

    fn accepts_dimension(&self, dimension: usize) -> bool {
        match self.dimension_hint() {
            Some(expected) => expected == dimension,
            None => true,
        }
    }
}

impl MemoryTextEmbedder for MemoryEmbeddingProvider {
    fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryVectorError> {
        match self {
            Self::Local(embedder) => embedder.embed(text),
            Self::OpenAiCompatible(embedder) => embedder.embed(text),
        }
    }
}

#[derive(Clone, Debug)]
struct LocalMemoryEmbedder {
    dimension: usize,
}

impl LocalMemoryEmbedder {
    fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

impl MemoryTextEmbedder for LocalMemoryEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryVectorError> {
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return Err(MemoryVectorError::EmbeddingUnavailable);
        }

        let mut vector = vec![0.0_f32; self.dimension];
        for token in &tokens {
            for (group_index, group) in SEMANTIC_GROUPS.iter().enumerate() {
                if group.iter().any(|term| *term == token.as_str()) {
                    vector[group_index % self.dimension] += 2.5;
                }
            }

            let hashed_index = SEMANTIC_GROUPS.len()
                + (stable_hash(token) as usize
                    % self.dimension.saturating_sub(SEMANTIC_GROUPS.len()).max(1));
            vector[hashed_index % self.dimension] += 1.0;
        }

        Ok(vector)
    }
}

#[derive(Clone, Debug)]
struct OpenAiCompatibleMemoryEmbedder {
    provider: String,
    model: String,
    base_url: String,
    api_key: Option<String>,
    dimension: usize,
    client: BlockingHttpClient,
}

struct OpenAiCompatibleEmbeddingConfig {
    provider: &'static str,
    default_base_url: &'static str,
    default_model: &'static str,
    default_dimension: usize,
    api_key_envs: &'static [&'static str],
    base_url_envs: &'static [&'static str],
    requires_key: bool,
    dimension: Option<usize>,
}

impl OpenAiCompatibleMemoryEmbedder {
    fn from_env(config: OpenAiCompatibleEmbeddingConfig) -> io::Result<Self> {
        let timeout = Duration::from_millis(timeout_millis_from_env()?);
        let api_key = first_non_empty_env_value(config.api_key_envs);
        if config.requires_key && api_key.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} must be configured for {} memory embeddings",
                    config.api_key_envs.join(" or "),
                    config.provider
                ),
            ));
        }
        let model = std::env::var("ANIMAOS_RS_MEMORY_EMBEDDING_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| config.default_model.to_string());
        let base_url = first_non_empty_env_value(config.base_url_envs)
            .unwrap_or_else(|| config.default_base_url.to_string());
        let dimension = config.dimension.unwrap_or(config.default_dimension);
        if dimension == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS must be greater than 0",
            ));
        }
        Self::new(
            config.provider,
            base_url,
            model,
            api_key,
            dimension,
            timeout,
        )
    }

    fn new(
        provider: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
        dimension: usize,
        timeout: Duration,
    ) -> io::Result<Self> {
        let client = BlockingHttpClient::builder()
            .timeout(timeout)
            .build()
            .map_err(http_client_error)?;
        Ok(Self {
            provider: provider.into(),
            model: model.into(),
            base_url: trim_base_url(base_url.into()),
            api_key,
            dimension,
            client,
        })
    }

    fn embeddings_url(&self) -> String {
        format!("{}/embeddings", self.base_url)
    }
}

impl MemoryTextEmbedder for OpenAiCompatibleMemoryEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryVectorError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(MemoryVectorError::EmbeddingUnavailable);
        }

        let request = OpenAiEmbeddingRequest {
            model: &self.model,
            input: text,
        };
        let mut builder = self.client.post(self.embeddings_url()).json(&request);
        if let Some(api_key) = &self.api_key {
            builder = builder.bearer_auth(api_key);
        }

        let response = builder
            .send()
            .map_err(|_| MemoryVectorError::EmbeddingUnavailable)?;
        if !response.status().is_success() {
            return Err(MemoryVectorError::EmbeddingUnavailable);
        }
        let response = response
            .json::<OpenAiEmbeddingResponse>()
            .map_err(|_| MemoryVectorError::EmbeddingUnavailable)?;
        let embedding = response
            .data
            .into_iter()
            .next()
            .map(|data| data.embedding)
            .filter(|embedding| !embedding.is_empty())
            .ok_or(MemoryVectorError::EmbeddingUnavailable)?;
        if embedding.len() != self.dimension {
            return Err(MemoryVectorError::DimensionMismatch);
        }
        Ok(embedding)
    }
}

#[derive(Serialize)]
struct OpenAiEmbeddingRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingResponse {
    data: Vec<OpenAiEmbeddingData>,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Clone, Debug)]
struct SqliteMemoryEmbeddingStore {
    path: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
struct StoredEmbedding {
    memory_id: String,
    model: String,
    vector: Vec<f32>,
}

impl SqliteMemoryEmbeddingStore {
    fn new(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let store = Self { path };
        store.ensure_schema()?;
        Ok(store)
    }

    fn connection(&self) -> io::Result<Connection> {
        let connection = Connection::open(&self.path).map_err(sqlite_error)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(sqlite_error)?;
        Ok(connection)
    }

    fn ensure_schema(&self) -> io::Result<()> {
        self.connection()?
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS memory_embedding_schema (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS memory_embeddings (
                    memory_id TEXT PRIMARY KEY NOT NULL,
                    model TEXT NOT NULL,
                    dimension INTEGER NOT NULL,
                    vector_json TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_memory_embeddings_model
                    ON memory_embeddings(model);

                INSERT OR REPLACE INTO memory_embedding_schema(key, value)
                    VALUES ('version', '1');
                "#,
            )
            .map_err(sqlite_error)
    }

    fn load_all(&self) -> io::Result<Vec<StoredEmbedding>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT memory_id, model, dimension, vector_json FROM memory_embeddings ORDER BY memory_id",
            )
            .map_err(sqlite_error)?;
        let mut rows = statement.query([]).map_err(sqlite_error)?;
        let mut embeddings = Vec::new();

        while let Some(row) = rows.next().map_err(sqlite_error)? {
            let memory_id: String = row.get(0).map_err(sqlite_error)?;
            let model: String = row.get(1).map_err(sqlite_error)?;
            let dimension: usize = row
                .get::<_, i64>(2)
                .map_err(sqlite_error)?
                .try_into()
                .unwrap_or(0);
            let raw: String = row.get(3).map_err(sqlite_error)?;
            let Ok(vector) = serde_json::from_str::<Vec<f32>>(&raw) else {
                warn!(memory_id = %memory_id, "skipping malformed persisted memory embedding");
                continue;
            };
            if vector.len() != dimension || vector.iter().any(|value| !value.is_finite()) {
                warn!(memory_id = %memory_id, "skipping invalid persisted memory embedding");
                continue;
            }
            embeddings.push(StoredEmbedding {
                memory_id,
                model,
                vector,
            });
        }

        Ok(embeddings)
    }

    fn upsert(&self, memory_id: &str, model: &str, vector: &[f32]) -> io::Result<()> {
        let vector_json = serde_json::to_string(vector).map_err(json_error)?;
        self.connection()?
            .execute(
                r#"
                INSERT INTO memory_embeddings(memory_id, model, dimension, vector_json, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(memory_id) DO UPDATE SET
                    model = excluded.model,
                    dimension = excluded.dimension,
                    vector_json = excluded.vector_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    memory_id,
                    model,
                    vector.len() as i64,
                    vector_json,
                    now_millis().to_string(),
                ],
            )
            .map(|_| ())
            .map_err(sqlite_error)
    }

    fn delete_many(&self, memory_ids: &[String]) -> io::Result<usize> {
        let connection = self.connection()?;
        let mut removed = 0;
        for memory_id in memory_ids {
            removed += connection
                .execute(
                    "DELETE FROM memory_embeddings WHERE memory_id = ?1",
                    params![memory_id],
                )
                .map_err(sqlite_error)?;
        }
        Ok(removed)
    }

    fn delete_stale(&self, valid_ids: &HashSet<String>) -> io::Result<usize> {
        let stale_ids = self
            .load_all()?
            .into_iter()
            .filter(|embedding| !valid_ids.contains(&embedding.memory_id))
            .map(|embedding| embedding.memory_id)
            .collect::<Vec<_>>();
        self.delete_many(&stale_ids)
    }
}

fn env_path(name: &'static str) -> io::Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must not be empty"),
        ));
    }
    Ok(Some(PathBuf::from(value)))
}

fn embedding_dimension_from_env() -> io::Result<Option<usize>> {
    let Some(value) = std::env::var_os("ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS") else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS must not be empty",
        ));
    }
    let value = value.to_string_lossy();
    let dimension = value.parse::<usize>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS must be a positive integer",
        )
    })?;
    if dimension == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS must be greater than 0",
        ));
    }
    Ok(Some(dimension))
}

fn timeout_millis_from_env() -> io::Result<u64> {
    let Some(value) = std::env::var_os("ANIMAOS_RS_MEMORY_EMBEDDINGS_TIMEOUT_MS") else {
        return Ok(DEFAULT_EMBEDDING_TIMEOUT_MS);
    };
    if value.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ANIMAOS_RS_MEMORY_EMBEDDINGS_TIMEOUT_MS must not be empty",
        ));
    }
    let timeout = value.to_string_lossy().parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "ANIMAOS_RS_MEMORY_EMBEDDINGS_TIMEOUT_MS must be a positive integer",
        )
    })?;
    if timeout == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ANIMAOS_RS_MEMORY_EMBEDDINGS_TIMEOUT_MS must be greater than 0",
        ));
    }
    Ok(timeout)
}

fn first_non_empty_env_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn trim_base_url(mut base_url: String) -> String {
    while base_url.ends_with('/') {
        base_url.pop();
    }
    base_url
}

fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            current.push(character.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(normalize_token(&current));
            current.clear();
        }
    }
    if !current.is_empty() {
        tokens.push(normalize_token(&current));
    }
    tokens
}

fn normalize_token(token: &str) -> String {
    for suffix in ["ing", "ed", "es", "s"] {
        if token.len() > suffix.len() + 3 && token.ends_with(suffix) {
            return token[..token.len() - suffix.len()].to_string();
        }
    }
    token.to_string()
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn sqlite_error(error: rusqlite::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn json_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn http_client_error(error: reqwest::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error)
}

static SEMANTIC_GROUPS: &[&[&str]] = &[
    &[
        "release", "ship", "launch", "deploy", "delivery", "publish", "rollout",
    ],
    &[
        "brief",
        "briefing",
        "summary",
        "summar",
        "note",
        "notes",
        "changelog",
        "report",
    ],
    &["concise", "terse", "short", "compact", "succinct", "tight"],
    &["preference", "prefer", "like", "style", "want", "wants"],
    &["rollback", "risk", "fallback", "revert", "recovery"],
    &["billing", "invoice", "ledger", "finance", "payment"],
    &[
        "latency",
        "performance",
        "speed",
        "slow",
        "fast",
        "throughput",
    ],
    &["memory", "remember", "recall", "fact", "context"],
    &["agent", "assistant", "worker", "planner", "critic"],
    &["user", "operator", "human", "customer"],
];

#[cfg(test)]
mod tests {
    use std::fs::remove_file;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::*;
    use anima_memory::{MemoryScope, MemoryType};

    static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn local_embedder_connects_semantic_release_words() {
        let mut runtime = MemoryEmbeddingRuntime::local_default();
        let memory = test_memory("memory-1", "Operator wants concise ship notes");

        runtime
            .upsert_memory(&memory)
            .expect("embedding should index");
        let hits = runtime.search("release briefing style", 1);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory_id, "memory-1");
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn sqlite_store_reloads_vectors_and_rebuilds_missing() {
        let path = temp_sqlite_path("reload");
        let _ = remove_file(&path);
        let memories = vec![
            test_memory("memory-1", "Operator wants concise ship notes"),
            test_memory("memory-2", "Billing ledger exports include invoice IDs"),
        ];

        let mut first = MemoryEmbeddingRuntime::local(DEFAULT_EMBEDDING_DIMENSION);
        first.store = Some(SqliteMemoryEmbeddingStore::new(path.clone()).expect("store opens"));
        let report = first
            .rebuild_from_memories(&memories)
            .expect("first rebuild should work");
        assert_eq!(report.rebuilt_vectors, 2);

        let mut second = MemoryEmbeddingRuntime::local(DEFAULT_EMBEDDING_DIMENSION);
        second.store = Some(SqliteMemoryEmbeddingStore::new(path.clone()).expect("store opens"));
        let report = second
            .rebuild_from_memories(&memories)
            .expect("second rebuild should work");
        assert_eq!(report.loaded_vectors, 2);
        assert_eq!(report.rebuilt_vectors, 0);
        assert_eq!(second.search("invoice ledger", 1)[0].memory_id, "memory-2");

        let _ = remove_file(&path);
    }

    #[test]
    fn sqlite_store_skips_corrupt_vectors_and_rebuilds() {
        let path = temp_sqlite_path("corrupt");
        let _ = remove_file(&path);
        let store = SqliteMemoryEmbeddingStore::new(path.clone()).expect("store opens");
        store
            .connection()
            .expect("connection opens")
            .execute(
                "INSERT INTO memory_embeddings(memory_id, model, dimension, vector_json, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params!["memory-1", LOCAL_MODEL, DEFAULT_EMBEDDING_DIMENSION as i64, "not-json", "1"],
            )
            .expect("corrupt row inserts");

        let mut runtime = MemoryEmbeddingRuntime::local(DEFAULT_EMBEDDING_DIMENSION);
        runtime.store = Some(store);
        let report = runtime
            .rebuild_from_memories(&[test_memory("memory-1", "Operator wants concise ship notes")])
            .expect("rebuild should repair corrupt row");

        assert_eq!(report.loaded_vectors, 0);
        assert_eq!(report.rebuilt_vectors, 1);
        assert_eq!(
            runtime.search("release briefing style", 1)[0].memory_id,
            "memory-1"
        );

        let _ = remove_file(&path);
    }

    #[test]
    fn sqlite_store_removes_stale_vectors() {
        let path = temp_sqlite_path("stale");
        let _ = remove_file(&path);
        let store = SqliteMemoryEmbeddingStore::new(path.clone()).expect("store opens");
        store
            .upsert("stale", LOCAL_MODEL, &[1.0, 0.0, 0.0])
            .expect("stale vector inserts");
        let mut runtime = MemoryEmbeddingRuntime::local(DEFAULT_EMBEDDING_DIMENSION);
        runtime.store = Some(store);

        let report = runtime
            .rebuild_from_memories(&[test_memory("memory-1", "Operator wants concise ship notes")])
            .expect("rebuild should remove stale row");

        assert_eq!(report.removed_stale_vectors, 1);
        let loaded = runtime
            .store
            .as_ref()
            .unwrap()
            .load_all()
            .expect("load works");
        assert!(loaded
            .iter()
            .all(|embedding| embedding.memory_id != "stale"));

        let _ = remove_file(&path);
    }

    #[test]
    fn openai_compatible_embedder_posts_embeddings_request() {
        let (base_url, requests) = spawn_embedding_server(vec![
            r#"{"data":[{"embedding":[1.0,0.0,0.0]}]}"#,
            r#"{"data":[{"embedding":[1.0,0.0,0.0]}]}"#,
        ]);
        let embedder = OpenAiCompatibleMemoryEmbedder::new(
            OPENAI_COMPATIBLE_PROVIDER,
            &base_url,
            "text-embedding-test",
            Some("test-key".into()),
            3,
            Duration::from_secs(5),
        )
        .expect("embedder should construct");
        let mut runtime = MemoryEmbeddingRuntime::from_provider(
            MemoryEmbeddingProvider::OpenAiCompatible(embedder),
        );

        runtime
            .upsert_memory(&test_memory(
                "memory-1",
                "Operator wants concise release notes",
            ))
            .expect("provider should index memory");
        let hits = runtime.search("release briefing style", 1);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory_id, "memory-1");
        assert_eq!(runtime.status().provider, OPENAI_COMPATIBLE_PROVIDER);
        assert_eq!(runtime.status().model, "text-embedding-test");
        assert_eq!(runtime.status().dimension, 3);
        let requests = requests.lock().expect("requests should be available");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("POST /v1/embeddings HTTP/1.1"));
        assert!(requests[0].contains("authorization: Bearer test-key"));
        assert!(requests[0].contains("text-embedding-test"));
        assert!(requests[1].contains("release briefing style"));
    }

    #[test]
    fn env_configures_ollama_openai_compatible_embeddings() {
        let _env_lock = ENV_LOCK.lock().expect("env lock should not poison");
        let (base_url, _requests) =
            spawn_embedding_server(vec![r#"{"data":[{"embedding":[0.0,1.0,0.0,0.0]}]}"#]);
        let guards = [
            EnvGuard::set("ANIMAOS_RS_MEMORY_EMBEDDINGS", OLLAMA_PROVIDER),
            EnvGuard::set("ANIMAOS_RS_MEMORY_EMBEDDINGS_BASE_URL", &base_url),
            EnvGuard::set("ANIMAOS_RS_MEMORY_EMBEDDING_MODEL", "nomic-test"),
            EnvGuard::set("ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS", "4"),
        ];

        let mut runtime = MemoryEmbeddingRuntime::from_env(None)
            .expect("ollama embedding runtime should configure");
        runtime
            .upsert_memory(&test_memory("memory-1", "semantic memory"))
            .expect("ollama-compatible provider should index");

        let status = runtime.status();
        assert_eq!(status.provider, OLLAMA_PROVIDER);
        assert_eq!(status.model, "nomic-test");
        assert_eq!(status.dimension, 4);
        drop(guards);
    }

    #[test]
    fn env_rejects_openai_embeddings_without_api_key() {
        let _env_lock = ENV_LOCK.lock().expect("env lock should not poison");
        let guards = [
            EnvGuard::set("ANIMAOS_RS_MEMORY_EMBEDDINGS", OPENAI_PROVIDER),
            EnvGuard::unset("ANIMAOS_RS_MEMORY_EMBEDDINGS_API_KEY"),
            EnvGuard::unset("OPENAI_API_KEY"),
            EnvGuard::unset("OPENAI_KEY"),
            EnvGuard::unset("OPENAI_TOKEN"),
        ];

        let error = match MemoryEmbeddingRuntime::from_env(None) {
            Ok(_) => panic!("openai provider should require an API key"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("OPENAI_API_KEY"));
        drop(guards);
    }

    #[test]
    fn env_rejects_too_small_dimension() {
        let _env_lock = ENV_LOCK.lock().expect("env lock should not poison");
        let guards = [
            EnvGuard::set("ANIMAOS_RS_MEMORY_EMBEDDINGS", LOCAL_PROVIDER),
            EnvGuard::set("ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS", "4"),
        ];

        let error = match MemoryEmbeddingRuntime::from_env(None) {
            Ok(_) => panic!("small embedding dimension should be rejected"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        drop(guards);
    }

    fn test_memory(id: &str, content: &str) -> Memory {
        Memory {
            id: id.to_string(),
            agent_id: "agent-1".into(),
            agent_name: "Planner".into(),
            memory_type: MemoryType::Fact,
            content: content.to_string(),
            importance: 0.8,
            created_at: 1,
            tags: None,
            scope: MemoryScope::Private,
            room_id: None,
            world_id: None,
            session_id: None,
        }
    }

    fn temp_sqlite_path(label: &str) -> PathBuf {
        let suffix = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("anima-memory-embeddings-{label}-{suffix}.sqlite"))
    }

    fn spawn_embedding_server(responses: Vec<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener.local_addr().expect("listener should have addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);

        thread::spawn(move || {
            for response_body in responses {
                let (mut stream, _) = listener.accept().expect("request should arrive");
                let mut buffer = [0_u8; 8192];
                let bytes_read = stream.read(&mut buffer).expect("request should read");
                let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
                server_requests
                    .lock()
                    .expect("requests lock should not poison")
                    .push(request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("response should write");
            }
        });

        (format!("http://{address}/v1"), requests)
    }

    struct EnvGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }

        fn unset(name: &'static str) -> Self {
            let previous = std::env::var_os(name);
            std::env::remove_var(name);
            Self { name, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }
}
