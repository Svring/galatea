#[derive(Clone, Debug)]
pub struct McpServiceDefinition {
    pub id: String,                      // Unique ID for routing, e.g., "project_api_mcp"
    pub name: String,                    // User-friendly name, e.g., "Project API MCP"
    pub port: u16,                       // Port the MCP server is running on
    pub openapi_spec_path_on_mcp: String, // The relative path to the OpenAPI spec on the MCP server itself (e.g., "/openapi.json")
} 