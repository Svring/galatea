use crate::parser_mod::structs::CodeEntity;
use anyhow::{Context, Result};
use async_openai::{
    config::OpenAIConfig,
    error::OpenAIError,
    types::CreateEmbeddingRequestArgs,
    Client as OpenAIClient,
};
use backoff::{future::retry_notify, Error as BackoffError, ExponentialBackoff};
use futures::future::join_all;
use futures::stream::{self, StreamExt};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, error, info, warn};

// Default embedding model
const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
// Concurrent requests limit
const CONCURRENT_REQUESTS: usize = 10;
const MAX_RETRY_DURATION_SECONDS: u64 = 120; // 2 minutes

/// Generates embeddings for entities in memory and returns the updated vector.
///
/// # Arguments
///
/// * `entities` - Input vector of `CodeEntity`.
/// * `model_name` - Optional name of the OpenAI embedding model to use.
/// * `api_key` - Optional OpenAI API key.
/// * `api_base` - Optional OpenAI API base URL.
///
/// # Returns
///
/// A `Result` containing the `Vec<CodeEntity>` with added embeddings, or an error.
pub async fn generate_embeddings(
    mut entities: Vec<CodeEntity>, // Take ownership and make mutable
    model_name: Option<String>,
    api_key: Option<String>,
    api_base: Option<String>,
) -> Result<Vec<CodeEntity>> {
    if entities.is_empty() {
        info!(target: "galatea::embedder", "No entities provided. Nothing to embed.");
        return Ok(entities);
    }
    // No need to load from file

    // 2. Initialize OpenAI Client
    let effective_api_key = api_key.or_else(|| std::env::var("OPENAI_API_KEY").ok());
    let effective_api_base = api_base.or_else(|| std::env::var("OPENAI_API_BASE").ok());
    
    let mut config = OpenAIConfig::default();
    if let Some(key) = effective_api_key {
        config = config.with_api_key(key);
    }
    if let Some(base) = effective_api_base {
         config = config.with_api_base(base);
    }
    
    // Only create client if needed
    if entities.iter().any(|e| e.embedding.is_none()) { 
        let client = OpenAIClient::with_config(config);
        let model = model_name.unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string());
        info!(target: "galatea::embedder", count = entities.len(), model_name = %model, "Generating embeddings for entities");

        // 3. Prepare data and generate embeddings concurrently with retry logic
        let results = stream::iter(entities.iter_mut())
            .filter_map(|entity| async move {
                if entity.embedding.is_none() && !entity.context.snippet.trim().is_empty() {
                    Some(entity)
                } else {
                    None
                }
            })
            .map(|entity| { // Closure for each entity
                let client_ref = &client;
                let snippet = entity.context.snippet.clone();
                let entity_name = entity.name.clone();
                let model_name = model.clone();
                
                async move { // Async block for the operation + retry
                    let operation = || async {
                        let request = CreateEmbeddingRequestArgs::default()
                            .model(model_name.clone())
                            .input(vec![snippet.clone()])
                            .build()
                            .map_err(|build_err| {
                                BackoffError::Permanent(OpenAIError::InvalidArgument(build_err.to_string()))
                            })?;
                        
                        client_ref.embeddings().create(request).await.map_err(|api_err| {
                            if is_rate_limit_error(&api_err) {
                                BackoffError::transient(api_err)
                            } else {
                                BackoffError::permanent(api_err)
                            }
                        })
                    }; // End of operation closure

                    let mut backoff_strategy = ExponentialBackoff::default();
                    backoff_strategy.max_elapsed_time = Some(Duration::from_secs(MAX_RETRY_DURATION_SECONDS));

                    let notify = |err: OpenAIError, dur: Duration| {
                        warn!(target: "galatea::embedder", entity_name = %entity_name, retry_duration = ?dur, error = ?err, "Rate limit error. Retrying.");
                    };

                    // Execute with retry
                    match retry_notify(backoff_strategy, operation, notify).await {
                        Ok(res) => {
                            if let Some(embedding_data) = res.data.into_iter().next() {
                                Ok((entity, Some(embedding_data.embedding)))
                            } else {
                                warn!(target: "galatea::embedder", entity_name = %entity_name, "No embedding data received");
                                Ok((entity, None))
                            }
                        }
                        Err(err) => {
                            error!(target: "galatea::embedder", entity_name = %entity_name, error = %err, "Failed to get embedding after retries. Skipping.");
                            Ok((entity, None)) // Treat final failure as skippable for this entity
                        }
                    } // End of match retry_notify
                } // CORRECT End of async move block
            }) // CORRECT End of .map()
            .buffer_unordered(CONCURRENT_REQUESTS)
            .collect::<Vec<Result<(&mut CodeEntity, Option<Vec<f32>>)>>>()
            .await;

        // 4. Update entities with embeddings (handle potential errors)
        let mut build_errors = 0;
        for result in results {
            match result {
                Ok((entity, embedding_opt)) => {
                    if let Some(embedding) = embedding_opt {
                        entity.embedding = Some(embedding);
                    }
                }
                Err(e) => {
                    error!(target: "galatea::embedder", error = ?e, "Embedding processing error (request build failed)");
                    build_errors += 1;
                }
            }
        }
        if build_errors > 0 { warn!(target: "galatea::embedder", count = build_errors, "Errors encountered during embedding request building."); }
        info!(target: "galatea::embedder", "Embedding generation finished.");
    } else {
         info!(target: "galatea::embedder", "All entities already have embeddings. Skipping generation.");
    }

    // No need to serialize or save - return the modified vector
    Ok(entities)
}

// Simplified rate limit check
fn is_rate_limit_error(err: &OpenAIError) -> bool {
    match err {
        OpenAIError::ApiError(api_err) => {
            matches!(api_err.code.as_deref(), Some("rate_limit_exceeded"))
        }
        OpenAIError::Reqwest(_) => true, 
        _ => false, 
    }
} 

async fn get_embedding_with_retry(
    client: &OpenAIClient<OpenAIConfig>,
    model_name: String,
    snippet: String,
    entity_name: String,
) -> Result<Option<Vec<f32>>> {
    let operation = || async {
        let request = CreateEmbeddingRequestArgs::default()
            .model(model_name.clone())
            .input(vec![snippet.clone()])
            .build()
            .map_err(|build_err| {
                BackoffError::Permanent(OpenAIError::InvalidArgument(build_err.to_string()))
            })?;
        
        client.embeddings().create(request).await.map_err(|api_err| {
            if is_rate_limit_error(&api_err) {
                BackoffError::transient(api_err)
            } else {
                BackoffError::permanent(api_err)
            }
        })
    };

    let mut backoff_strategy = ExponentialBackoff::default();
    backoff_strategy.max_elapsed_time = Some(Duration::from_secs(MAX_RETRY_DURATION_SECONDS));

    let notify = |err: OpenAIError, dur: Duration| {
        warn!(target: "galatea::embedder", entity_name = %entity_name, retry_duration = ?dur, error = ?err, "Rate limit error for get_embedding_with_retry. Retrying.");
    };

    match retry_notify(backoff_strategy, operation, notify).await {
        Ok(res) => {
            if let Some(embedding_data) = res.data.into_iter().next() {
                Ok(Some(embedding_data.embedding))
            } else {
                warn!(target: "galatea::embedder", entity_name = %entity_name, "No embedding data received (get_embedding_with_retry).");
                Ok(None)
            }
        }
        Err(e) => {
            error!(target: "galatea::embedder", entity_name = %entity_name, error = %e, "Failed to get embedding after retries (get_embedding_with_retry). Skipping.");
            Err(anyhow::anyhow!("Failed to get embedding for entity '{}': {}", entity_name, e))
        },
    }
}

async fn generate_embeddings_core(
    mut entities: Vec<CodeEntity>,
    model_name_opt: Option<String>,
    api_key_opt: Option<String>,
    api_base_opt: Option<String>,
) -> Result<Vec<CodeEntity>> {
    let effective_api_key = api_key_opt.or_else(|| std::env::var("OPENAI_API_KEY").ok());
    let effective_api_base = api_base_opt.or_else(|| std::env::var("OPENAI_API_BASE").ok());
    
    let mut openai_config = OpenAIConfig::default();
    if let Some(key) = effective_api_key {
        openai_config = openai_config.with_api_key(key);
    } else {
        if entities.iter().any(|e| e.embedding.is_none() && !e.context.snippet.trim().is_empty()) {
            return Err(anyhow::anyhow!("OpenAI API key not found. Set OPENAI_API_KEY env var or provide --api-key."));
        }
        // If no entities need embedding, we can return early without a client.
        if !entities.iter().any(|e| e.embedding.is_none() && !e.context.snippet.trim().is_empty()) {
            info!(target: "galatea::embedder", "All entities already have embeddings or snippets are empty. Skipping generation (core).");
            return Ok(entities);
        }
    }
    if let Some(base) = effective_api_base { 
        openai_config = openai_config.with_api_base(base); 
    }

    let client = OpenAIClient::with_config(openai_config);
    let model = model_name_opt.unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string());

    let mut futures_to_run = Vec::new();
    // Store indices of entities that will be processed
    let mut processing_indices = Vec::new(); 

    for (index, entity) in entities.iter().enumerate() {
        if entity.embedding.is_none() && !entity.context.snippet.trim().is_empty() {
            processing_indices.push(index);
            futures_to_run.push(get_embedding_with_retry(
                &client, // Pass client by reference
                model.clone(),
                entity.context.snippet.clone(),
                entity.name.clone(),
            ));
        } 
    }
    
    if futures_to_run.is_empty() {
        info!(target: "galatea::embedder", "No entities require embedding generation.");
        return Ok(entities);
    }
    info!(target: "galatea::embedder", count = futures_to_run.len(), model_name = %model, "Generating embeddings for entities (core)");

    let results = join_all(futures_to_run).await;
    let mut update_count = 0;

    for (i, result) in results.into_iter().enumerate() {
        let entity_index = processing_indices[i]; // Get original entity index
        match result {
            Ok(Some(embedding_vector)) => {
                entities[entity_index].embedding = Some(embedding_vector);
                update_count += 1;
            }
            Ok(None) => {
                // Successfully processed but no embedding data (already logged in get_embedding_with_retry)
            }
            Err(e) => {
                // Error already logged by map_embedding_error or get_embedding_with_retry
                error!(target: "galatea::embedder", entity_name = %entities[entity_index].name, error = %e, "Final error for entity. Embedding not updated.");
            }
        }
    }
    
    info!(target: "galatea::embedder", count = update_count, "Embedding generation finished. Updated entities.");
    Ok(entities)
}

// Public function for file-based operation
pub async fn generate_embeddings_for_index(
    input_path: &Path,
    output_path: &Path,
    model_name: Option<String>,
    api_key: Option<String>,
    api_base: Option<String>,
) -> Result<()> {
    info!(target: "galatea::embedder", input_path = %input_path.display(), output_path = %output_path.display(), "Starting embedding generation for file index.");

    let input_json = fs::read_to_string(input_path)
        .with_context(|| format!("Failed to read input file: {}", input_path.display()))?;
    let entities_vec: Vec<CodeEntity> = serde_json::from_str(&input_json)
        .with_context(|| format!("Failed to deserialize input JSON from: {}", input_path.display()))?;
    if entities_vec.is_empty() { 
        info!(target: "galatea::embedder", "No entities found in input file.");
        return Ok(()); 
    }

    let entities_with_embeddings = generate_embeddings_core(
        entities_vec, 
        model_name, 
        api_key, 
        api_base
    ).await?;

    let output_json = serde_json::to_string_pretty(&entities_with_embeddings)
        .context("Failed to serialize updated entities to JSON")?;
    info!(target: "galatea::embedder", output_path = %output_path.display(), "Saving updated entities to file.");
    let mut file = fs::File::create(output_path)
        .with_context(|| format!("Failed to create output file: {}", output_path.display()))?;
    file.write_all(output_json.as_bytes())
        .with_context(|| format!("Failed to write to output file: {}", output_path.display()))?;

    info!(target: "galatea::embedder", output_path = %output_path.display(), "Embedding process complete.");
    Ok(())
}

// Public function for in-memory operation
pub async fn generate_embeddings_for_vec(
    entities: Vec<CodeEntity>,
    model_name: Option<String>,
    api_key: Option<String>,
    api_base: Option<String>,
) -> Result<Vec<CodeEntity>> {
    info!(target: "galatea::embedder", "Generating embeddings for a vector of CodeEntity objects.");
    generate_embeddings_core(entities, model_name, api_key, api_base).await
} 

