use crate::parser_mod::structs::CodeEntity;
use anyhow::{Context, Result};
use async_openai::{
    config::OpenAIConfig, types::CreateEmbeddingRequestArgs, Client as OpenAIClient,
};
use qdrant_client::qdrant::{
    vectors_config::Config, CreateCollectionBuilder, Distance, PointStruct, SearchPointsBuilder,
    UpsertPointsBuilder, VectorParamsBuilder, VectorsConfig,
};
use qdrant_client::Payload;
use qdrant_client::Qdrant;
use serde_json::json; // Import json macro
use std::convert::TryFrom; // Needed for Payload::try_from
use std::fs;
use std::path::Path;
use tracing::{debug, error, info, warn}; // Added tracing import
use uuid::Uuid;

// Define dimension for OpenAI text-embedding-3-small
const EMBEDDING_DIMENSION: u64 = 1536;
const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small"; // Use constant from embedder

pub async fn create_collection(collection_name: &str, qdrant_url: &str) -> Result<()> {
    let client = Qdrant::from_url(qdrant_url).build()?;

    // Check if collection already exists (optional, but good practice)
    if client.collection_exists(collection_name).await? {
        info!(target: "galatea::hoarder", collection_name = %collection_name, "Collection already exists.");
        return Ok(());
    }

    info!(target: "galatea::hoarder", collection_name = %collection_name, "Creating collection.");
    // Explicitly create VectorParams first
    let vector_params = VectorParamsBuilder::new(EMBEDDING_DIMENSION, Distance::Cosine).build();
    // Then create VectorsConfig using these params
    let vectors_config = VectorsConfig {
        config: Some(Config::Params(vector_params)),
    };

    client
        .create_collection(
            CreateCollectionBuilder::new(collection_name).vectors_config(vectors_config), // Pass the constructed VectorsConfig
        )
        .await?;

    Ok(())
}

// Internal core logic for upserting entities from a Vec
async fn upsert_entities_core(
    collection_name: &str,
    entities: Vec<CodeEntity>,
    client: &Qdrant, // Accept client reference
) -> Result<()> {
    let mut points_to_upsert = Vec::new();
    info!(target: "galatea::hoarder", count = entities.len(), "Preparing entities for upsert.");

    for entity in entities {
        if let Some(vector) = entity.embedding {
            let payload_value = json!({
                "name": entity.name,
                "signature": entity.signature,
                "code_type": entity.code_type,
                "docstring": entity.docstring,
                "line": entity.line,
                "line_from": entity.line_from,
                "line_to": entity.line_to,
                "context": entity.context
            });
            let payload = match Payload::try_from(payload_value) {
                Ok(p) => p,
                Err(e) => {
                    warn!(target: "galatea::hoarder", entity_name = %entity.name, error = ?e, "Failed to convert entity to payload. Skipping.");
                    continue;
                }
            };
            let point_id = Uuid::new_v4().to_string();
            points_to_upsert.push(PointStruct::new(point_id, vector, payload));
        } else {
            debug!(target: "galatea::hoarder", entity_name = %entity.name, "Skipping entity due to missing embedding.");
        }
    }

    if points_to_upsert.is_empty() {
        info!(target: "galatea::hoarder", "No valid points with embeddings found to upsert.");
        return Ok(());
    }

    info!(target: "galatea::hoarder", count = points_to_upsert.len(), collection_name = %collection_name, "Upserting points into collection.");
    let response =
        client // Use passed client reference
            .upsert_points(UpsertPointsBuilder::new(collection_name, points_to_upsert))
            .await?;

    info!(target: "galatea::hoarder", "Upsert finished.");
    debug!(target: "galatea::hoarder", ?response, "Upsert response details.");
    Ok(())
}

// Public function for file-based operation (Original name)
pub async fn upsert_embeddings(
    collection_name: &str,
    json_file_path: &Path,
    qdrant_url: &str,
) -> Result<()> {
    let client = Qdrant::from_url(qdrant_url).build()?;
    info!(target: "galatea::hoarder", path = %json_file_path.display(), "Reading embeddings from file.");
    let json_content = fs::read_to_string(json_file_path)
        .with_context(|| format!("Failed to read file: {}", json_file_path.display()))?;
    let entities: Vec<CodeEntity> = serde_json::from_str(&json_content)
        .with_context(|| format!("Failed to parse JSON from: {}", json_file_path.display()))?;

    // Call core logic
    upsert_entities_core(collection_name, entities, &client).await
}

// Public function for in-memory operation
pub async fn upsert_entities_from_vec(
    collection_name: &str,
    entities: Vec<CodeEntity>,
    qdrant_url: &str,
) -> Result<()> {
    let client = Qdrant::from_url(qdrant_url).build()?;
    // Call core logic
    upsert_entities_core(collection_name, entities, &client).await
}

// Refined query function
pub async fn query(
    collection_name: &str,
    query: &str,
    model_name: Option<String>,
    api_key: Option<String>,
    api_base: Option<String>,
    qdrant_url: &str,
) -> Result<Vec<CodeEntity>> {
    // --- OpenAI Client Setup (similar to embedder.rs) ---
    let effective_api_key = api_key.or_else(|| std::env::var("OPENAI_API_KEY").ok());
    let effective_api_base = api_base.or_else(|| std::env::var("OPENAI_API_BASE").ok());

    let mut config = OpenAIConfig::default();
    // Require API key for querying
    let key = effective_api_key
        .context("OpenAI API key not found. Set OPENAI_API_KEY env var or use --api-key.")?;
    config = config.with_api_key(key);

    if let Some(base) = effective_api_base {
        config = config.with_api_base(base);
    }
    let openai_client = OpenAIClient::with_config(config);
    let model = model_name.unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string());
    // --- End OpenAI Client Setup ---

    info!(target: "galatea::hoarder", query = %query, model_name = %model, "Generating embedding for query.");

    // --- Create Embedding Request ---
    let request = CreateEmbeddingRequestArgs::default()
        .model(model)
        .input(vec![query.to_string()]) // Pass query string directly
        .build()
        .with_context(|| format!("Failed to create embedding request for query: {}", query))?;

    // --- Get Embedding (with basic error handling, no retry for now) ---
    let query_embedding_response = openai_client
        .embeddings()
        .create(request)
        .await
        .with_context(|| format!("OpenAI API call failed for query: {}", query))?;

    let query_embedding = query_embedding_response
        .data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .context("No embedding data received from OpenAI API")?;
    info!(target: "galatea::hoarder", "Query embedding generated successfully.");
    // --- End Embedding Generation ---

    // --- Qdrant Client and Search ---
    info!(target: "galatea::hoarder", collection_name = %collection_name, "Connecting to Qdrant and searching collection.");
    let client = Qdrant::from_url(qdrant_url).build()?;

    let search_request = SearchPointsBuilder::new(collection_name, query_embedding, 10) // Limit to 10 results for API
        .with_payload(true) // Include payload in results
        .build();

    let response = client
        .search_points(search_request)
        .await
        .with_context(|| format!("Qdrant search failed in collection '{}'", collection_name))?;
    // --- End Qdrant Search ---

    let mut entities: Vec<CodeEntity> = Vec::new();
    if response.result.is_empty() {
        info!(target: "galatea::hoarder", query = %query, "No results found for query.");
    } else {
        info!(target: "galatea::hoarder", count = response.result.len(), query = %query, "Found results for query.");
        for point in response.result {
            // Convert payload to JSON value using serde_json
            match serde_json::to_value(&point.payload) {
                Ok(json_value) => {
                    // Try to deserialize the payload back into a CodeEntity
                    match serde_json::from_value::<CodeEntity>(json_value.clone()) {
                        Ok(mut entity) => {
                            // Optionally, include score or other info from point if needed
                            // For now, just reconstruct the entity. Embedding is not stored in payload by default.
                            // If embedding needs to be returned, it should be handled here.
                            entity.embedding = None; // Clear any potentially stale embedding from payload if it was there.
                            entities.push(entity);
                        }
                        Err(e) => {
                            error!(target: "galatea::hoarder", error = ?e, payload = %json_value, "Failed to deserialize payload to CodeEntity.");
                        }
                    }
                }
                Err(e) => {
                    error!(target: "galatea::hoarder", error = ?e, payload = ?point.payload, "Failed to convert Qdrant payload to JSON value.");
                }
            }
        }
    }

    Ok(entities) // Return the collected entities
}
