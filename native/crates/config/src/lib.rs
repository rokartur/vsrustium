//! Settings as platform/configuration and workbench/services/configuration hold them: the registered
//! defaults, the user and workspace `settings.json` files parsed with the JSONC rules of base/common/json.ts,
//! and the merged view extensions and the workbench read.

pub mod configuration;
pub mod indentation;
pub mod json;
pub mod json_edit;
pub mod model;
pub mod registry;
pub mod service;
pub mod workspace;
