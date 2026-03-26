#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::dbg_macro)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

pub mod ast;
pub mod cache;
pub mod embeddings;
pub mod eval;
pub mod cluster;
pub mod config;
pub mod entrypoint;
pub mod flow;
pub mod git;
pub mod graph;
pub mod ir;
pub mod llm;
pub mod output;
pub mod pipeline;
pub mod query_engine;
pub mod rank;
pub mod types;
