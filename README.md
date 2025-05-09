# Galatea Code Processing and Indexing Tool

This tool provides functionalities to find, parse, process, embed, and index code files from your projects. It interacts with Tree-sitter for parsing, OpenAI for embeddings, and Qdrant for vector storage.

## Installation & Setup

1.  **Clone the repository.**
2.  **Install Rust:** Follow the instructions at [rustup.rs](https://rustup.rs/).
3.  **Build:** Navigate to the project directory and run `cargo build --release`. The executable will be located at `target/release/galatea`.
4.  **Environment Variables (Optional but Recommended):**
    *   Set `OPENAI_API_KEY`: Your OpenAI API key is needed for embedding generation and querying.
    *   Set `OPENAI_API_BASE` (Optional): If you use a proxy or non-standard base URL for OpenAI.
    *   Ensure a Qdrant instance is running and accessible (default URL used is `http://localhost:6334`).

## Usage

The tool is invoked via `cargo run -- <COMMAND> [OPTIONS]` or directly using the compiled binary `./target/release/galatea <COMMAND> [OPTIONS]`.

### Commands

Here are the available commands and their options:

---

#### `find-files`

Recursively finds files within a directory that match the given suffixes, excluding specified directory names. Prints the list of found file paths to standard output.

**Arguments:**

*   `--dir <PATH>`: (Required) The directory path to start searching from.
*   `--suffixes <SUFFIX1,SUFFIX2,...>`: (Required) Comma-separated list of file suffixes to match (e.g., `rs,ts,tsx`).
*   `--exclude-dirs <DIR1,DIR2,...>`: (Optional) Comma-separated list of directory names to exclude.
    *   **Default:** `node_modules,target,dist,build,.git,.vscode,.idea`

**Example:**

```bash
# Find all .rs and .toml files in the current directory, excluding default dirs + 'examples'
cargo run -- find-files --dir . --suffixes rs,toml --exclude-dirs node_modules,target,dist,build,.git,.vscode,.idea,examples

# Find all .ts files in './src' using only default exclusions
cargo run -- find-files --dir ./src --suffixes ts
```

---

#### `parse-directory`

Recursively finds files (respecting exclusions), parses them according to their extension (Rust, TS, TSX), processes the extracted code entities (splitting large snippets, merging based on granularity), and prints the resulting array of `CodeEntity` objects as JSON to standard output.

**Arguments:**

*   `--dir <PATH>`: (Required) Directory to search recursively. Defaults to the current directory (`.`).
*   `--suffixes <SUFFIX1,SUFFIX2,...>`: (Required) Comma-separated list of file suffixes to parse (e.g., `rs,ts,tsx`).
*   `--exclude-dirs <DIR1,DIR2,...>`: (Optional) Comma-separated list of directory names to exclude.
    *   **Default:** `node_modules,target,dist,build,.git,.vscode,.idea`
*   `--max-snippet-size <NUMBER>`: (Optional) Maximum snippet size in characters. Entities exceeding this will be split into chunks.
*   `--granularity <LEVEL>`: (Optional) Controls how aggressively consecutive entities are merged.
    *   `fine` (Default): Merges only consecutive Imports, Constants, Variables if under `max_snippet_size`.
    *   `medium`: Merges any consecutive entities if the result is <= `max_snippet_size / 2`. Requires `--max-snippet-size`.
    *   `coarse`: Merges any consecutive entities if the result is <= `max_snippet_size`. Requires `--max-snippet-size`.

**Example:**

```bash
# Parse all .rs files in ./src, use fine granularity, output to stdout
cargo run -- parse-directory --dir ./src --suffixes rs

# Parse .ts/.tsx files, limit snippets to 1000 chars, use coarse merging, output to stdout
cargo run -- parse-directory --dir ./src --suffixes ts,tsx --max-snippet-size 1000 --granularity coarse

# Parse and save output to a file using redirection
cargo run -- parse-directory --dir ./src --suffixes rs > initial_index.json
```

---

#### `generate-embeddings`

Reads a JSON file containing `CodeEntity` objects (likely produced by `parse-directory`), generates OpenAI embeddings for the `snippet` field of each entity, adds the embedding to the object, and saves the result to a new JSON file. Skips entities that already have embeddings.

**Arguments:**

*   `--input-file <PATH>`: (Required) Path to the input JSON file (output from `parse-directory`).
*   `--output-file <PATH>`: (Required) Path to save the output JSON file with added embeddings.
*   `--model <MODEL_NAME>`: (Optional) OpenAI embedding model name.
    *   **Default:** `text-embedding-3-small`
*   `--api-key <KEY>`: (Optional) OpenAI API key. Overrides `OPENAI_API_KEY` environment variable.
*   `--api-base <URL>`: (Optional) OpenAI API base URL. Overrides `OPENAI_API_BASE` environment variable.

**Example:**

```bash
# Generate embeddings for 'initial_index.json', save to 'index_with_embeddings.json'
# Assumes OPENAI_API_KEY is set in the environment
cargo run -- generate-embeddings --input-file initial_index.json --output-file index_with_embeddings.json

# Use a specific model and API key
cargo run -- generate-embeddings --input-file initial_index.json --output-file index_with_embeddings.json --model text-embedding-ada-002 --api-key sk-...
```

---

#### `upsert-embeddings`

Reads a JSON file containing `CodeEntity` objects *with* embeddings (the output of `generate-embeddings`), creates a Qdrant collection if it doesn't exist (with correct dimensions for `text-embedding-3-small`), and upserts the entities (embedding vector + metadata payload) into the collection.

**Arguments:**

*   `--input-file <PATH>`: (Required) Path to the input JSON file containing entities *with* embeddings.
*   `--collection-name <NAME>`: (Required) Name of the Qdrant collection to create/upsert into.

**Example:**

```bash
# Upsert data from 'index_with_embeddings.json' into the 'my-code-index' collection
cargo run -- upsert-embeddings --input-file index_with_embeddings.json --collection-name my-code-index
```

---

#### `query`

Embeds a given text query using OpenAI and performs a similarity search against a specified Qdrant collection, printing the top results (including payload).

**Arguments:**

*   `--collection-name <NAME>`: (Required) Name of the Qdrant collection to query.
*   `--model <MODEL_NAME>`: (Optional) OpenAI embedding model name for the query.
    *   **Default:** `text-embedding-3-small`
*   `--api-key <KEY>`: (Optional) OpenAI API key. Overrides `OPENAI_API_KEY`.
*   `--api-base <URL>`: (Optional) OpenAI API base URL. Overrides `OPENAI_API_BASE`.
*   `<QUERY_TEXT>`: (Required) Positional argument containing the text query to search for. Must be the *last* argument.

**Example:**

```bash
# Query the 'my-code-index' collection for "sidebar component"
# Assumes OPENAI_API_KEY is set
cargo run -- query --collection-name my-code-index "sidebar component"

# Query with specific options
cargo run -- query --collection-name my-code-index --api-key sk-... "how to handle state"

cargo run -- query --collection-name agent-index "sidebar component" --api-key sk-Yx2FiiiPuF9QS4CivU4Wqfr6SdtYaBgOJSeba9NqqRLEYicU --api-base https://aiproxy.usw.sealos.io/v1
```

---

#### `build-index`

Performs the full indexing pipeline in one command: Find Files -> Parse -> Process (Split/Merge) -> Generate Embeddings -> Store in Qdrant. This operates primarily in memory, avoiding intermediate files (except potentially during the embedding/hoarding steps if not fully refactored yet).

**Arguments:**

*   **Wanderer Args:**
    *   `--dir <PATH>`: (Required) Directory to search. Defaults to `.`.
    *   `--suffixes <SUFFIX1,...>`: (Required) File suffixes.
    *   `--exclude-dirs <DIR1,...>`: (Optional) Directories to exclude. Default: `node_modules,...`
*   **Processing Args:**
    *   `--max-snippet-size <NUMBER>`: (Optional) Max snippet size for splitting.
    *   `--granularity <LEVEL>`: (Optional) Merging granularity (`fine`, `medium`, `coarse`). Default: `fine`.
*   **Embedder Args:**
    *   `--embedding-model <MODEL_NAME>`: (Optional) Embedding model. Default: `text-embedding-3-small`.
    *   `--api-key <KEY>`: (Optional) OpenAI key. Uses env var if omitted.
    *   `--api-base <URL>`: (Optional) OpenAI base URL. Uses env var if omitted.
*   **Hoarder Args:**
    *   `--collection-name <NAME>`: (Required) Qdrant collection name.

**Example:**

```bash
# Build a 'fine' index for Rust code in ./src, store in 'rust-project-index' collection
# Assumes OPENAI_API_KEY is set
cargo run -- build-index --dir ./src --suffixes rs --collection-name rust-project-index

cargo run -- build-index --dir /Users/linkling/Code/agent --suffixes ts,tsx --collection-name agent-index --max-snippet-size 2000 --granularity medium --api-key sk-Yx2FiiiPuF9QS4CivU4Wqfr6SdtYaBgOJSeba9NqqRLEYicU --api-base https://aiproxy.usw.sealos.io/v1

# Build a 'coarse' index for TS/TSX in ., limit snippets, store in 'ts-project-index'
# Assumes OPENAI_API_KEY is set
cargo run -- build-index --dir . --suffixes ts,tsx --collection-name ts-project-index --granularity coarse --max-snippet-size 2000
```

---