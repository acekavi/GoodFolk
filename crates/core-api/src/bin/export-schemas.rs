//! Writes `openapi.json` and `schema.graphql` into the given directory for frontend codegen.

use anyhow::Context;
use core_api::graphql::build_schema;
use core_api::openapi::ApiDoc;
use std::path::PathBuf;
use utoipa::OpenApi;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).context("usage: export-schemas <output-dir>")?);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("openapi.json"), ApiDoc::openapi().to_pretty_json()?)?;
    std::fs::write(dir.join("schema.graphql"), build_schema(false).sdl())?;
    Ok(())
}
