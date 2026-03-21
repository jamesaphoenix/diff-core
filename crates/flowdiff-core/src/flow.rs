//! Data flow tracing and heuristic inference module.
//!
//! Analyzes parsed files to infer additional data flow edges beyond what
//! static import/call analysis can determine. Uses pattern matching on
//! call sites and identifiers to detect:
//!
//! - Database persistence patterns (`.save()`, `.insert()`, `INSERT INTO`)
//! - Database read patterns (`.find()`, `.query()`, `SELECT`)
//! - Event emission (`.emit()`, `.publish()`, `.dispatch()`)
//! - Event handling (`.on()`, `.subscribe()`, `.listen()`)
//! - Configuration reads (`process.env`, `os.environ`)
//! - HTTP outbound calls (`fetch()`, `axios.get()`)
//! - Logging calls (`console.log`, `logger.info`)
//!
//! Also detects frameworks from import patterns.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use aho_corasick::AhoCorasick;

use crate::ast::{CallSite, ParsedFile};
use crate::graph::{GraphEdge, SymbolGraph};
use crate::ir::IrFile;
use crate::types::EdgeType;

/// A data flow pattern detected via heuristic matching.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FlowPattern {
    /// Database write: `.save()`, `.insert()`, `.create()`, `.update()`, `.delete()`, `INSERT INTO`
    Persistence,
    /// Database read: `.find()`, `.query()`, `.select()`, `.findOne()`, `SELECT`
    DatabaseRead,
    /// Event emission: `.emit()`, `.publish()`, `.send()`, `.dispatch()`
    EventEmission,
    /// Event handling: `.on()`, `.subscribe()`, `.listen()`, `.addEventListener()`
    EventHandling,
    /// Configuration read: `process.env`, `os.environ`, `config.get()`
    ConfigRead,
    /// HTTP outbound call: `fetch()`, `axios.get()`, `requests.get()`
    HttpCall,
    /// Logging: `console.log`, `logger.info`, `logging.debug`
    Logging,
}

/// A heuristic edge inferred from code patterns.
#[derive(Debug, Clone, PartialEq)]
pub struct HeuristicEdge {
    /// Symbol id of the function containing the pattern (e.g. `file.ts::handler`)
    pub from_symbol: String,
    /// The file containing the pattern
    pub file: String,
    /// The detected flow pattern
    pub pattern: FlowPattern,
    /// Confidence score [0.0, 1.0]
    pub confidence: f64,
    /// The callee string that matched (evidence)
    pub evidence: String,
    /// Line number where the pattern was detected
    pub line: usize,
}

/// Result of data flow analysis across all files.
#[derive(Debug, Clone)]
pub struct FlowAnalysis {
    /// Heuristic edges inferred from code patterns.
    pub heuristic_edges: Vec<HeuristicEdge>,
    /// Frameworks detected from import patterns.
    pub frameworks_detected: Vec<String>,
}

/// A data flow edge connecting a producer function to a consumer function
/// through a shared variable within the same function scope.
///
/// Example: `const x = funcA(); funcB(x)` creates an edge from funcA → funcB via "x".
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DataFlowEdge {
    /// Callee of the assignment (the function producing data).
    pub producer: String,
    /// Callee of the consuming call (the function receiving the data).
    pub consumer: String,
    /// Variable name connecting the producer to the consumer.
    pub via: String,
    /// Symbol ID of the function containing both calls.
    pub containing_function: String,
    /// File path.
    pub file: String,
    /// Line of the consumer call.
    pub line: usize,
}

/// Configuration for flow analysis.
#[derive(Debug, Clone)]
pub struct FlowConfig {
    /// Maximum call chain depth to trace (prevents runaway on cycles).
    pub max_depth: usize,
}

impl Default for FlowConfig {
    fn default() -> Self {
        Self { max_depth: 10 }
    }
}

// ---------------------------------------------------------------------------
// Heuristic pattern matching rules
// ---------------------------------------------------------------------------

/// Persistence (database write) patterns.
const DB_WRITE_METHODS: &[&str] = &[
    ".save",
    ".insert",
    ".create",
    ".update",
    ".delete",
    ".remove",
    ".upsert",
    ".bulkCreate",
    ".bulkInsert",
    ".insertMany",
    ".updateMany",
    ".deleteMany",
    ".findAndUpdate",
    ".findOneAndUpdate",
    ".findOneAndDelete",
    ".findOneAndRemove",
    ".persist",
    ".flush",
    ".execute",
    ".run",
];

/// SQL write keywords (case-insensitive matching on string literals).
const SQL_WRITE_KEYWORDS: &[&str] = &[
    "INSERT INTO",
    "UPDATE ",
    "DELETE FROM",
    "DROP TABLE",
    "ALTER TABLE",
    "CREATE TABLE",
    "TRUNCATE",
];

/// Database read patterns.
const DB_READ_METHODS: &[&str] = &[
    ".find",
    ".findOne",
    ".findById",
    ".findAll",
    ".findMany",
    ".findFirst",
    ".findUnique",
    ".query",
    ".select",
    ".get",
    ".fetch",
    ".count",
    ".aggregate",
    ".groupBy",
    ".where",
];

/// SQL read keywords.
const SQL_READ_KEYWORDS: &[&str] = &["SELECT ", "SELECT\n"];

/// Event emission patterns.
const EVENT_EMIT_METHODS: &[&str] = &[
    ".emit",
    ".publish",
    ".send",
    ".dispatch",
    ".fire",
    ".trigger",
    ".broadcast",
    ".notify",
    ".produce",
    ".enqueue",
];

/// Event handling patterns.
const EVENT_HANDLE_METHODS: &[&str] = &[
    ".on",
    ".subscribe",
    ".listen",
    ".addEventListener",
    ".addListener",
    ".handle",
    ".consume",
    ".onMessage",
    ".onEvent",
];

/// Config read patterns.
const CONFIG_PATTERNS: &[&str] = &[
    "process.env",
    "os.environ",
    "os.getenv",
    "config.get",
    "config.set",
    "dotenv",
    "Deno.env",
];

/// HTTP outbound call patterns.
const HTTP_CALL_PATTERNS: &[&str] = &[
    "fetch",
    "axios.get",
    "axios.post",
    "axios.put",
    "axios.delete",
    "axios.patch",
    "axios.request",
    "requests.get",
    "requests.post",
    "requests.put",
    "requests.delete",
    "requests.patch",
    "http.get",
    "http.post",
    "http.request",
    "urllib.request",
    "httpx.get",
    "httpx.post",
];

/// Logging patterns.
const LOG_PATTERNS: &[&str] = &[
    "console.log",
    "console.error",
    "console.warn",
    "console.info",
    "console.debug",
    "console.trace",
    "logger.info",
    "logger.error",
    "logger.warn",
    "logger.debug",
    "logger.trace",
    "logger.fatal",
    "logging.info",
    "logging.error",
    "logging.warning",
    "logging.debug",
    "logging.critical",
    "log.info",
    "log.error",
    "log.warn",
    "log.debug",
];


// ---------------------------------------------------------------------------
// Framework detection
// ---------------------------------------------------------------------------

/// Known framework import sources and their display names.
const FRAMEWORK_IMPORTS: &[(&str, &str)] = &[
    // JavaScript/TypeScript
    ("express", "Express"),
    ("fastify", "Fastify"),
    ("next", "Next.js"),
    ("next/", "Next.js"),
    ("react", "React"),
    ("react-dom", "React"),
    ("vue", "Vue"),
    ("@angular/core", "Angular"),
    ("svelte", "Svelte"),
    ("@nestjs/common", "NestJS"),
    ("@nestjs/core", "NestJS"),
    ("hono", "Hono"),
    ("koa", "Koa"),
    ("@effect/", "Effect.ts"),
    ("effect", "Effect.ts"),
    ("prisma", "Prisma"),
    ("@prisma/client", "Prisma"),
    ("typeorm", "TypeORM"),
    ("sequelize", "Sequelize"),
    ("mongoose", "Mongoose"),
    ("drizzle-orm", "Drizzle"),
    ("@trpc/server", "tRPC"),
    ("@trpc/client", "tRPC"),
    ("graphql", "GraphQL"),
    ("@apollo/server", "Apollo"),
    ("@apollo/client", "Apollo"),
    ("tailwindcss", "Tailwind CSS"),
    ("redux", "Redux"),
    ("@reduxjs/toolkit", "Redux"),
    ("zustand", "Zustand"),
    ("zod", "Zod"),
    ("vitest", "Vitest"),
    ("jest", "Jest"),
    ("@effect/vitest", "Effect.ts"),
    // Python
    ("fastapi", "FastAPI"),
    ("flask", "Flask"),
    ("django", "Django"),
    ("sqlalchemy", "SQLAlchemy"),
    ("pydantic", "Pydantic"),
    ("celery", "Celery"),
    ("pytest", "pytest"),
    ("alembic", "Alembic"),
    ("tortoise", "Tortoise ORM"),
    ("starlette", "Starlette"),
    ("aiohttp", "aiohttp"),
    ("httpx", "httpx"),
    ("uvicorn", "Uvicorn"),
    // Go
    ("net/http", "Go net/http"),
    ("github.com/gin-gonic/gin", "Gin"),
    ("github.com/labstack/echo", "Echo"),
    ("github.com/go-chi/chi", "Chi"),
    ("github.com/gofiber/fiber", "Fiber"),
    ("github.com/gorilla/mux", "Gorilla Mux"),
    ("google.golang.org/grpc", "gRPC"),
    ("github.com/spf13/cobra", "Cobra"),
    ("github.com/spf13/viper", "Viper"),
    ("gorm.io/gorm", "GORM"),
    ("github.com/jmoiron/sqlx", "sqlx"),
    ("database/sql", "Go database/sql"),
    ("github.com/go-playground/validator", "Go Validator"),
    ("github.com/stretchr/testify", "Testify"),
    // Rust
    ("actix_web", "Actix-web"),
    ("actix-web", "Actix-web"),
    ("axum", "Axum"),
    ("rocket", "Rocket"),
    ("warp", "Warp"),
    ("hyper", "Hyper"),
    ("tokio", "Tokio"),
    ("diesel", "Diesel"),
    ("sqlx", "SQLx"),
    ("sea_orm", "SeaORM"),
    ("sea-orm", "SeaORM"),
    ("clap", "Clap"),
    ("tauri", "Tauri"),
    ("serde", "Serde"),
    ("tower", "Tower"),
    ("tonic", "Tonic"),
    ("tracing", "Tracing"),
    // Java
    ("org.springframework.boot", "Spring Boot"),
    ("org.springframework.web", "Spring MVC"),
    ("org.springframework.data", "Spring Data"),
    ("org.springframework.stereotype", "Spring Boot"),
    ("org.springframework.beans", "Spring Boot"),
    ("org.springframework.context", "Spring Boot"),
    ("org.springframework.security", "Spring Security"),
    ("jakarta.persistence", "JPA"),
    ("javax.persistence", "JPA"),
    ("jakarta.ws.rs", "JAX-RS"),
    ("javax.ws.rs", "JAX-RS"),
    ("jakarta.servlet", "Servlet"),
    ("javax.servlet", "Servlet"),
    ("org.hibernate", "Hibernate"),
    ("org.junit", "JUnit"),
    ("org.junit.jupiter", "JUnit 5"),
    ("org.mockito", "Mockito"),
    ("com.google.inject", "Guice"),
    ("io.micronaut", "Micronaut"),
    ("io.quarkus", "Quarkus"),
    ("org.apache.maven", "Maven"),
    // C#
    ("Microsoft.AspNetCore", "ASP.NET Core"),
    ("Microsoft.AspNetCore.Mvc", "ASP.NET Core MVC"),
    ("Microsoft.AspNetCore.Builder", "ASP.NET Core"),
    ("Microsoft.AspNetCore.Http", "ASP.NET Core"),
    ("Microsoft.AspNetCore.Routing", "ASP.NET Core"),
    ("Microsoft.AspNetCore.Authorization", "ASP.NET Core"),
    ("Microsoft.AspNetCore.Identity", "ASP.NET Identity"),
    ("Microsoft.AspNetCore.SignalR", "SignalR"),
    ("Microsoft.EntityFrameworkCore", "Entity Framework Core"),
    ("Microsoft.Extensions.DependencyInjection", "ASP.NET Core"),
    ("Microsoft.Extensions.Logging", "ASP.NET Core"),
    ("Microsoft.Extensions.Configuration", "ASP.NET Core"),
    ("System.Linq", "LINQ"),
    ("Xunit", "xUnit"),
    ("NUnit", "NUnit"),
    ("Microsoft.VisualStudio.TestTools", "MSTest"),
    ("Moq", "Moq"),
    ("FluentAssertions", "FluentAssertions"),
    ("MediatR", "MediatR"),
    ("AutoMapper", "AutoMapper"),
    ("Newtonsoft.Json", "Newtonsoft.Json"),
    ("System.Text.Json", "System.Text.Json"),
    ("Dapper", "Dapper"),
    ("Microsoft.AspNetCore.Components", "Blazor"),
    // PHP (use namespace segments without trailing backslash;
    // the match logic adds \ as a separator)
    ("Illuminate", "Laravel"),
    ("Illuminate\\Http", "Laravel"),
    ("Illuminate\\Routing", "Laravel"),
    ("Illuminate\\Database", "Laravel Eloquent"),
    ("Illuminate\\Queue", "Laravel Queue"),
    ("Illuminate\\Console", "Laravel Artisan"),
    ("Illuminate\\Support", "Laravel"),
    ("Laravel", "Laravel"),
    ("Symfony", "Symfony"),
    ("Symfony\\Component\\HttpFoundation", "Symfony"),
    ("Symfony\\Component\\Console", "Symfony Console"),
    ("Symfony\\Component\\Routing", "Symfony"),
    ("Doctrine\\ORM", "Doctrine ORM"),
    ("Doctrine\\DBAL", "Doctrine DBAL"),
    ("Slim", "Slim"),
    ("GuzzleHttp", "Guzzle"),
    ("Monolog", "Monolog"),
    ("PHPUnit", "PHPUnit"),
    ("Livewire", "Livewire"),
    ("Inertia", "Inertia"),
    // Ruby
    ("rails", "Rails"),
    ("action_controller", "Rails"),
    ("active_record", "Rails ActiveRecord"),
    ("active_support", "Rails"),
    ("action_view", "Rails"),
    ("action_mailer", "Rails"),
    ("active_job", "Rails ActiveJob"),
    ("active_storage", "Rails"),
    ("action_cable", "Rails ActionCable"),
    ("sinatra", "Sinatra"),
    ("rack", "Rack"),
    ("grape", "Grape"),
    ("hanami", "Hanami"),
    ("rspec", "RSpec"),
    ("minitest", "Minitest"),
    ("sidekiq", "Sidekiq"),
    ("devise", "Devise"),
    ("pundit", "Pundit"),
    ("cancancan", "CanCanCan"),
    ("sequel", "Sequel"),
    ("mongoid", "Mongoid"),
    ("dry-rb", "dry-rb"),
    ("roda", "Roda"),
    ("puma", "Puma"),
    ("faraday", "Faraday"),
    ("httparty", "HTTParty"),
    ("factory_bot", "FactoryBot"),
    ("rubocop", "RuboCop"),
    // Kotlin
    ("io.ktor", "Ktor"),
    ("io.ktor.server", "Ktor"),
    ("io.ktor.client", "Ktor Client"),
    ("io.ktor.routing", "Ktor"),
    ("org.springframework", "Spring Boot"),
    ("org.springframework.boot", "Spring Boot"),
    ("org.springframework.web", "Spring MVC"),
    ("org.springframework.data", "Spring Data"),
    ("org.jetbrains.exposed", "Exposed"),
    ("org.jetbrains.compose", "Jetpack Compose"),
    ("androidx.compose", "Jetpack Compose"),
    ("kotlinx.coroutines", "Kotlin Coroutines"),
    ("kotlinx.serialization", "Kotlin Serialization"),
    ("org.junit", "JUnit"),
    ("kotlin.test", "Kotlin Test"),
    ("io.kotest", "Kotest"),
    ("io.mockk", "MockK"),
    ("org.koin", "Koin"),
    ("com.squareup.retrofit2", "Retrofit"),
    ("com.squareup.okhttp3", "OkHttp"),
    ("io.arrow-kt", "Arrow"),
    ("com.google.dagger", "Dagger/Hilt"),
    // Swift
    ("SwiftUI", "SwiftUI"),
    ("UIKit", "UIKit"),
    ("Foundation", "Foundation"),
    ("Vapor", "Vapor"),
    ("Fluent", "Fluent"),
    ("FluentPostgresDriver", "Fluent"),
    ("FluentSQLiteDriver", "Fluent"),
    ("FluentMySQLDriver", "Fluent"),
    ("XCTest", "XCTest"),
    ("Combine", "Combine"),
    ("CoreData", "Core Data"),
    ("SwiftData", "SwiftData"),
    ("Alamofire", "Alamofire"),
    ("Kitura", "Kitura"),
    ("Perfect", "Perfect"),
    ("Hummingbird", "Hummingbird"),
    ("Observation", "Observation"),
    ("SwiftNIO", "SwiftNIO"),
    ("GRDB", "GRDB"),
    ("SnapKit", "SnapKit"),
    ("Quick", "Quick"),
    ("Nimble", "Nimble"),
    // C
    ("stdio.h", "C stdio"),
    ("stdlib.h", "C stdlib"),
    ("string.h", "C string"),
    ("pthread.h", "POSIX threads"),
    ("unistd.h", "POSIX"),
    ("curl/curl.h", "libcurl"),
    ("sqlite3.h", "SQLite3"),
    ("mysql.h", "MySQL C API"),
    ("libpq-fe.h", "PostgreSQL libpq"),
    ("openssl/ssl.h", "OpenSSL"),
    ("jansson.h", "Jansson"),
    ("cjson/cJSON.h", "cJSON"),
    ("check.h", "Check"),
    ("cmocka.h", "CMocka"),
    // C++
    ("iostream", "C++ STL"),
    ("vector", "C++ STL"),
    ("memory", "C++ STL"),
    ("string", "C++ STL"),
    ("algorithm", "C++ STL"),
    ("thread", "C++ STL"),
    ("mutex", "C++ STL"),
    ("boost/asio.hpp", "Boost.Asio"),
    ("boost/beast.hpp", "Boost.Beast"),
    ("boost/", "Boost"),
    ("crow.h", "Crow"),
    ("crow/crow.h", "Crow"),
    ("httplib.h", "cpp-httplib"),
    ("pistache/endpoint.h", "Pistache"),
    ("pistache/", "Pistache"),
    ("drogon/drogon.h", "Drogon"),
    ("drogon/", "Drogon"),
    ("cpprest/", "C++ REST SDK"),
    ("nlohmann/json.hpp", "nlohmann/json"),
    ("sqlite3.h", "SQLite3"),
    ("pqxx/pqxx", "libpqxx"),
    ("mysql++.h", "MySQL++"),
    ("gtest/gtest.h", "Google Test"),
    ("gmock/gmock.h", "Google Mock"),
    ("catch2/catch.hpp", "Catch2"),
    ("catch2/", "Catch2"),
    ("doctest/doctest.h", "doctest"),
    ("fmt/format.h", "fmt"),
    ("spdlog/spdlog.h", "spdlog"),
    ("grpcpp/grpcpp.h", "gRPC C++"),
    ("grpc++/", "gRPC C++"),
    ("absl/", "Abseil"),
    ("folly/", "Folly"),
    ("Qt", "Qt"),
    ("QApplication", "Qt"),
    ("QWidget", "Qt"),
    // Scala
    ("play.api.mvc", "Play Framework"),
    ("play.mvc", "Play Framework"),
    ("akka.actor", "Akka"),
    ("akka.stream", "Akka Streams"),
    ("akka.http", "Akka HTTP"),
    ("scala.concurrent", "Scala Concurrency"),
    ("org.scalatest", "ScalaTest"),
    ("org.specs2", "Specs2"),
    ("org.scalatestplus", "ScalaTestPlus"),
    ("org.mockito", "Mockito Scala"),
    ("slick", "Slick"),
    ("doobie", "Doobie"),
    ("quill", "Quill"),
    ("scalikejdbc", "ScalikeJDBC"),
    ("circe", "Circe"),
    ("spray", "Spray"),
    ("org.http4s", "http4s"),
    ("cats", "Cats"),
    ("cats.effect", "Cats Effect"),
    ("zio", "ZIO"),
    ("monix", "Monix"),
    ("fs2", "FS2"),
    ("shapeless", "Shapeless"),
    ("com.typesafe.config", "Typesafe Config"),
    ("io.getquill", "Quill"),
    ("sttp", "sttp"),
    ("tapir", "Tapir"),
];

// ---------------------------------------------------------------------------
// Pre-compiled pattern matchers (built once, reused across all files)
// ---------------------------------------------------------------------------

/// Suffix set for DB write methods (method name after last dot, e.g. "save").
fn db_write_suffix_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        DB_WRITE_METHODS
            .iter()
            .map(|s| s.trim_start_matches('.'))
            .collect()
    })
}

/// Suffix set for DB read methods.
fn db_read_suffix_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        DB_READ_METHODS
            .iter()
            .map(|s| s.trim_start_matches('.'))
            .collect()
    })
}

/// Suffix set for event emission methods.
fn event_emit_suffix_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        EVENT_EMIT_METHODS
            .iter()
            .map(|s| s.trim_start_matches('.'))
            .collect()
    })
}

/// Suffix set for event handling methods.
fn event_handle_suffix_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        EVENT_HANDLE_METHODS
            .iter()
            .map(|s| s.trim_start_matches('.'))
            .collect()
    })
}

/// Exact-match set for logging patterns.
fn log_pattern_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| LOG_PATTERNS.iter().copied().collect())
}

/// Exact-match set for known non-DB callees.
fn non_db_callee_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            "JSON.parse",
            "JSON.stringify",
            "Object.create",
            "Object.assign",
            "Array.from",
            "Promise.resolve",
            "Promise.reject",
            "Date.now",
            "Math.round",
            "Math.floor",
            "Math.ceil",
            "Math.abs",
            "Math.min",
            "Math.max",
        ]
        .into_iter()
        .collect()
    })
}

/// Exact-match set for known non-DB receivers (lowercased).
fn non_db_receiver_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            "array", "map", "set", "object", "string", "number", "promise", "json", "math",
            "date", "regexp", "cache", "localstorage", "sessionstorage", "window", "document",
            "navigator", "console", "process", "os", "path", "fs", "http", "https", "url",
            "buffer", "stream", "crypto", "util", "events", "child_process", "cluster", "net",
            "tls", "dns", "axios", "requests", "fetch", "httpx", "urllib", "list", "dict",
            "tuple", "frozenset", "deque", "defaultdict", "items", "result", "results", "data",
            "response", "request", "config", "env", "settings", "options", "args", "params",
            "logger", "log", "logging", "console",
        ]
        .into_iter()
        .collect()
    })
}

/// Aho-Corasick automaton for DB-keyword substring matching in receivers.
fn db_keyword_automaton() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| {
        AhoCorasick::new([
            "db", "database", "repo", "repository", "model", "store", "dao", "collection",
            "prisma", "sequelize", "typeorm", "mongoose", "drizzle", "session", "connection",
            "pool", "client", "table", "entity", "schema", "migration", "knex", "query", "sql",
        ])
        .expect("valid patterns")
    })
}

/// Aho-Corasick automaton for ORM-specific names in confidence scoring.
fn orm_automaton() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| {
        AhoCorasick::new([
            "prisma",
            "sequelize",
            "typeorm",
            "mongoose",
            "sqlalchemy",
            "drizzle",
        ])
        .expect("valid patterns")
    })
}

/// Aho-Corasick automaton for high-confidence receiver keywords in confidence scoring.
fn confidence_receiver_automaton() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| {
        AhoCorasick::new(["db", "repo", "model", "store", "dao", "collection"])
            .expect("valid patterns")
    })
}

/// Aho-Corasick automaton for SQL write keywords (lowercased).
fn sql_write_automaton() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| {
        let patterns: Vec<String> = SQL_WRITE_KEYWORDS
            .iter()
            .map(|k| k.to_lowercase())
            .collect();
        AhoCorasick::new(&patterns).expect("valid patterns")
    })
}

/// Aho-Corasick automaton for SQL read keywords (lowercased).
fn sql_read_automaton() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| {
        let patterns: Vec<String> = SQL_READ_KEYWORDS
            .iter()
            .map(|k| k.to_lowercase())
            .collect();
        AhoCorasick::new(&patterns).expect("valid patterns")
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Analyze data flow patterns across all parsed files.
///
/// Scans call sites for heuristic patterns (DB writes, event emission, config reads, etc.)
/// and detects frameworks from import patterns.
pub fn analyze_data_flow(files: &[ParsedFile], _config: &FlowConfig) -> FlowAnalysis {
    let mut heuristic_edges = Vec::new();

    for file in files {
        let file_edges = detect_heuristic_patterns(file);
        heuristic_edges.extend(file_edges);
    }

    let frameworks_detected = detect_frameworks(files);

    FlowAnalysis {
        heuristic_edges,
        frameworks_detected,
    }
}

/// Enrich an existing symbol graph with heuristic edges.
///
/// For each heuristic edge, adds the appropriate edge type (Writes, Reads, Emits, Handles)
/// from the containing symbol to the file's module node (since the target is typically
/// an external resource like a database or event bus).
pub fn enrich_graph(graph: &mut SymbolGraph, analysis: &FlowAnalysis) {
    for edge in &analysis.heuristic_edges {
        let edge_type = match edge.pattern {
            FlowPattern::Persistence => EdgeType::Writes,
            FlowPattern::DatabaseRead => EdgeType::Reads,
            FlowPattern::EventEmission => EdgeType::Emits,
            FlowPattern::EventHandling => EdgeType::Handles,
            FlowPattern::ConfigRead => EdgeType::Reads,
            FlowPattern::HttpCall => EdgeType::Reads,
            FlowPattern::Logging => continue, // Don't add graph edges for logging
        };

        let from_idx = match graph.get_node(&edge.from_symbol) {
            Some(idx) => idx,
            None => {
                // Try the file-level module node as fallback
                match graph.get_node(&edge.file) {
                    Some(idx) => idx,
                    None => continue,
                }
            }
        };

        // For heuristic edges, we connect to the file's module node since the actual
        // target (database, event bus, etc.) is external and not in our graph.
        let to_idx = match graph.get_node(&edge.file) {
            Some(idx) => idx,
            None => continue,
        };

        // Don't add self-edges
        if from_idx == to_idx {
            continue;
        }

        graph.add_edge(from_idx, to_idx, GraphEdge { edge_type });
    }
}

/// Detect frameworks from import patterns across all files.
pub fn detect_frameworks(files: &[ParsedFile]) -> Vec<String> {
    let mut frameworks: HashSet<String> = HashSet::new();

    for file in files {
        for import in &file.imports {
            let source = &import.source;
            for &(pattern, name) in FRAMEWORK_IMPORTS {
                // Match exact, or prefixed by separator: slash (JS/TS),
                // dot (Python), :: (Rust), or backslash (PHP namespaces)
                if source == pattern
                    || source.starts_with(pattern)
                        && source.as_bytes().get(pattern.len()).map_or(false, |&b| {
                            b == b'/' || b == b'.' || b == b':' || b == b'\\'
                        })
                {
                    frameworks.insert(name.to_string());
                }
            }
        }
    }

    // Also detect Next.js from file structure conventions
    for file in files {
        let path = &file.path;
        if path.contains("pages/") || path.contains("app/") {
            if path.ends_with("page.tsx")
                || path.ends_with("page.ts")
                || path.ends_with("page.jsx")
                || path.ends_with("page.js")
                || path.ends_with("route.ts")
                || path.ends_with("route.js")
                || path.ends_with("layout.tsx")
                || path.ends_with("layout.ts")
            {
                frameworks.insert("Next.js".to_string());
            }
        }
    }

    let mut result: Vec<String> = frameworks.into_iter().collect();
    result.sort();
    result
}

// ---------------------------------------------------------------------------
// Internal: heuristic pattern detection
// ---------------------------------------------------------------------------

/// Detect heuristic data flow patterns in a single file's call sites.
fn detect_heuristic_patterns(file: &ParsedFile) -> Vec<HeuristicEdge> {
    let mut edges = Vec::new();

    for call in &file.call_sites {
        if let Some(edge) = classify_call_site(call, &file.path) {
            edges.push(edge);
        }
    }

    edges
}

/// Classify a single call site into a flow pattern, if any.
///
/// Pattern matching order is important: more specific patterns are checked first
/// to avoid false positives (e.g., `axios.get` is HTTP, not a DB read).
fn classify_call_site(call: &CallSite, file_path: &str) -> Option<HeuristicEdge> {
    let callee = &call.callee;
    let containing = call
        .containing_function
        .as_ref()
        .map(|f| format!("{}::{}", file_path, f))
        .unwrap_or_else(|| file_path.to_string());

    let make_edge = |pattern: FlowPattern, confidence: f64| HeuristicEdge {
        from_symbol: containing.clone(),
        file: file_path.to_string(),
        pattern,
        confidence,
        evidence: callee.clone(),
        line: call.line,
    };

    // 1. Logging — most specific, check first to prevent console.log matching elsewhere
    if let Some(pattern) = match_logging(callee) {
        return Some(make_edge(pattern, 0.95));
    }

    // 2. Config reads — specific patterns like process.env, os.environ
    if let Some(pattern) = match_config_read(callee) {
        return Some(make_edge(pattern, 0.9));
    }

    // 3. HTTP calls — check before DB reads so axios.get/requests.get match HTTP
    if let Some(pattern) = match_http_call(callee) {
        return Some(make_edge(pattern, 0.85));
    }

    // 4. Check for collection/stdlib false positives before DB patterns
    if is_collection_method(callee) {
        return None;
    }

    // 5. Event emission
    if let Some(pattern) = match_event_emission(callee) {
        return Some(make_edge(pattern, 0.8));
    }

    // 6. Event handling
    if let Some(pattern) = match_event_handling(callee) {
        return Some(make_edge(pattern, 0.8));
    }

    // 7. Persistence (DB writes) — with DB-like receiver guard
    if let Some(pattern) = match_persistence(callee) {
        return Some(make_edge(pattern, confidence_for_db_pattern(callee)));
    }

    // 8. Database reads — with DB-like receiver guard
    if let Some(pattern) = match_db_read(callee) {
        return Some(make_edge(pattern, confidence_for_db_pattern(callee)));
    }

    None
}

/// Check if a callee is a standard library collection/utility method (not a DB operation).
fn is_collection_method(callee: &str) -> bool {
    if let Some(method) = callee.split('.').last() {
        match method {
            // Array/list methods
            "push" | "pop" | "shift" | "unshift" | "splice" | "slice" | "concat" | "join"
            | "reverse" | "sort" | "fill" | "copyWithin" | "flat" | "flatMap" | "map"
            | "filter" | "reduce" | "forEach" | "some" | "every" | "includes" | "indexOf"
            | "find" | "findIndex"
            // Python list/set
            | "append" | "extend" | "clear" | "copy" | "items" | "len"
            // Object/Map/Set methods
            | "keys" | "values" | "entries" | "toString" | "toLocaleString" | "has" | "add"
            // JSON/utility
            | "parse" | "stringify" | "assign" | "from" | "resolve" | "reject"
            | "now" | "round" | "floor" | "ceil" | "abs" | "min" | "max"
            | "charAt" | "charCodeAt" | "trim" | "split" | "replace" | "match"
            | "startsWith" | "endsWith" | "padStart" | "padEnd" | "repeat"
            | "toLowerCase" | "toUpperCase" => {
                return true;
            }
            _ => {}
        }
    }

    // Known non-DB full callee patterns (HashSet lookup)
    if non_db_callee_set().contains(callee) {
        return true;
    }

    false
}

fn match_persistence(callee: &str) -> Option<FlowPattern> {
    // Must be a method call (has a dot) with a DB-like receiver
    if let Some(method) = callee.rsplit('.').next() {
        if callee.contains('.') && db_write_suffix_set().contains(method) && has_db_like_receiver(callee) {
            return Some(FlowPattern::Persistence);
        }
    }

    // SQL keywords in the callee string (single-pass Aho-Corasick)
    let lower = callee.to_lowercase();
    if sql_write_automaton().is_match(&lower) {
        return Some(FlowPattern::Persistence);
    }

    None
}

fn match_db_read(callee: &str) -> Option<FlowPattern> {
    // Must be a method call with a DB-like receiver
    if let Some(method) = callee.rsplit('.').next() {
        if callee.contains('.') && db_read_suffix_set().contains(method) && has_db_like_receiver(callee) {
            return Some(FlowPattern::DatabaseRead);
        }
    }

    // SQL keywords (single-pass Aho-Corasick)
    let lower = callee.to_lowercase();
    if sql_read_automaton().is_match(&lower) {
        return Some(FlowPattern::DatabaseRead);
    }

    None
}

/// Check if the receiver (part before the last dot) looks like a database/ORM object.
///
/// Returns true for receivers containing DB-related keywords like "db", "repo",
/// "model", "store", "collection", "prisma", "session", etc.
/// Returns false for single-letter variables, known non-DB names, and stdlib objects.
fn has_db_like_receiver(callee: &str) -> bool {
    // Get the receiver (everything before the last method)
    let parts: Vec<&str> = callee.rsplitn(2, '.').collect();
    let receiver = if parts.len() == 2 { parts[1] } else { return false };
    let lower = receiver.to_lowercase();

    // Skip single-letter variable names (too ambiguous)
    if receiver.len() <= 1 {
        return false;
    }

    // Skip known non-DB receivers (HashSet lookup)
    if non_db_receiver_set().contains(lower.as_str()) {
        return false;
    }

    // Positive signal: receiver contains DB-related keywords (Aho-Corasick single-pass)
    if db_keyword_automaton().is_match(&lower) {
        return true;
    }

    // Also match if it looks like a specific ORM method chain (e.g., prisma.user)
    let first_part = callee.split('.').next().unwrap_or("");
    let first_lower = first_part.to_lowercase();
    if db_keyword_automaton().is_match(&first_lower) {
        return true;
    }

    // For multi-part receivers like "prisma.user", check the first part
    if receiver.contains('.') {
        let root = receiver.split('.').next().unwrap_or("");
        let root_lower = root.to_lowercase();
        if db_keyword_automaton().is_match(&root_lower) {
            return true;
        }
    }

    false
}

fn match_event_emission(callee: &str) -> Option<FlowPattern> {
    if let Some(method) = callee.rsplit('.').next() {
        if callee.contains('.') && event_emit_suffix_set().contains(method) {
            return Some(FlowPattern::EventEmission);
        }
    }
    None
}

fn match_event_handling(callee: &str) -> Option<FlowPattern> {
    if let Some(method) = callee.rsplit('.').next() {
        if callee.contains('.') && event_handle_suffix_set().contains(method) {
            return Some(FlowPattern::EventHandling);
        }
    }
    None
}

fn match_config_read(callee: &str) -> Option<FlowPattern> {
    // Fast check: process.env and os.environ are the most common config patterns
    if callee.starts_with("process.env") || callee.starts_with("os.environ") {
        return Some(FlowPattern::ConfigRead);
    }

    for &pattern in CONFIG_PATTERNS {
        // Match exact, dot-prefix (member access), or bracket-prefix
        if callee == pattern
            || callee.starts_with(pattern) && callee.as_bytes().get(pattern.len()).map_or(true, |&b| b == b'.' || b == b'[')
        {
            return Some(FlowPattern::ConfigRead);
        }
    }

    None
}

fn match_http_call(callee: &str) -> Option<FlowPattern> {
    for &pattern in HTTP_CALL_PATTERNS {
        if callee == pattern
            || callee.starts_with(pattern) && callee.as_bytes().get(pattern.len()) == Some(&b'.')
        {
            return Some(FlowPattern::HttpCall);
        }
    }
    None
}

fn match_logging(callee: &str) -> Option<FlowPattern> {
    if log_pattern_set().contains(callee) {
        Some(FlowPattern::Logging)
    } else {
        None
    }
}

/// Assign confidence based on how specific the DB pattern is.
fn confidence_for_db_pattern(callee: &str) -> f64 {
    let lower = callee.to_lowercase();

    // ORM-specific method chains are high confidence (Aho-Corasick single-pass)
    if orm_automaton().is_match(&lower) {
        return 0.95;
    }

    // Methods with "db" or "repo" or "repository" in the receiver are high confidence
    let receiver = callee.split('.').next().unwrap_or("");
    let lower_receiver = receiver.to_lowercase();
    if confidence_receiver_automaton().is_match(&lower_receiver) {
        return 0.9;
    }

    // SQL keywords are high confidence (Aho-Corasick single-pass)
    if sql_write_automaton().is_match(&lower) || sql_read_automaton().is_match(&lower) {
        return 0.95;
    }

    // Generic methods like `.save()` on unknown receivers are medium confidence
    0.7
}

/// Trace call chains to a configurable depth, collecting all reachable symbols.
///
/// Given a starting symbol, follows call edges in the graph up to `max_depth` hops.
/// Returns the list of symbol IDs reachable from the start, in BFS order.
pub fn trace_call_chain(graph: &SymbolGraph, start: &str, max_depth: usize) -> Vec<String> {
    use std::collections::VecDeque;

    let start_idx = match graph.get_node(start) {
        Some(idx) => idx,
        None => return vec![],
    };

    let mut visited: HashSet<petgraph::graph::NodeIndex> = HashSet::new();
    let mut queue: VecDeque<(petgraph::graph::NodeIndex, usize)> = VecDeque::new();
    let mut result = Vec::new();

    visited.insert(start_idx);
    queue.push_back((start_idx, 0));

    while let Some((current, depth)) = queue.pop_front() {
        if depth > 0 {
            result.push(graph.graph[current].id.clone());
        }

        if depth >= max_depth {
            continue;
        }

        // Follow outgoing Calls edges
        for neighbor in graph
            .graph
            .neighbors_directed(current, petgraph::Direction::Outgoing)
        {
            if visited.insert(neighbor) {
                // Check if the edge is a Calls edge
                if let Some(edge) = graph.graph.find_edge(current, neighbor) {
                    if graph.graph[edge].edge_type == EdgeType::Calls {
                        queue.push_back((neighbor, depth + 1));
                    }
                }
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Full data flow tracing
// ---------------------------------------------------------------------------

/// Build data flow edges from extracted data flow info for a single file.
///
/// Connects variable assignments from calls to subsequent calls that use those
/// variables as arguments within the same function scope.
///
/// Example: in `function f() { const x = funcA(); funcB(x); }`,
/// produces `DataFlowEdge { producer: "funcA", consumer: "funcB", via: "x" }`.
pub fn build_data_flow_edges(
    info: &crate::ast::DataFlowInfo,
    file_path: &str,
) -> Vec<DataFlowEdge> {
    use std::collections::HashMap;

    let mut edges = Vec::new();

    // Group assignments by containing function for scope-aware matching.
    let mut assignments_by_scope: HashMap<Option<&str>, Vec<&crate::ast::VarCallAssignment>> =
        HashMap::new();
    for assignment in &info.assignments {
        let key = assignment.containing_function.as_deref();
        assignments_by_scope.entry(key).or_default().push(assignment);
    }

    // For each call, check if any argument matches a variable assigned from another call.
    for call in &info.calls_with_args {
        let scope_key = call.containing_function.as_deref();
        if let Some(scope_assignments) = assignments_by_scope.get(&scope_key) {
            for arg in &call.arguments {
                for assignment in scope_assignments {
                    if assignment.variable == *arg && assignment.callee != call.callee {
                        let containing = match &call.containing_function {
                            Some(f) => format!("{}::{}", file_path, f),
                            None => file_path.to_string(),
                        };
                        edges.push(DataFlowEdge {
                            producer: assignment.callee.clone(),
                            consumer: call.callee.clone(),
                            via: arg.clone(),
                            containing_function: containing,
                            file: file_path.to_string(),
                            line: call.line,
                        });
                    }
                }
            }
        }
    }

    edges
}

/// Trace data flow across all files, producing edges that show how data moves
/// through variable assignments and function calls.
///
/// Requires source code for each file (re-parses with tree-sitter for finer-grained
/// extraction of variable assignments and call arguments).
pub fn trace_data_flow(files_with_source: &[(&str, &str)]) -> Vec<DataFlowEdge> {
    let mut all_edges = Vec::new();

    for &(path, source) in files_with_source {
        match crate::ast::extract_data_flow_info(path, source) {
            Ok(info) => {
                let edges = build_data_flow_edges(&info, path);
                all_edges.extend(edges);
            }
            Err(_) => continue,
        }
    }

    all_edges
}

// ---------------------------------------------------------------------------
// IR-based public API
// ---------------------------------------------------------------------------

/// Analyze data flow patterns from IR files (declarative query engine / IR path).
///
/// Delegates to the existing heuristic analysis via ParsedFile conversion.
/// The heuristic pattern matching operates on the same call site data available
/// in both representations.
pub fn analyze_data_flow_ir(files: &[IrFile], config: &FlowConfig) -> FlowAnalysis {
    let parsed: Vec<ParsedFile> = files.iter().map(|f| f.to_parsed_file()).collect();
    analyze_data_flow(&parsed, config)
}

/// Detect frameworks from IR files' import patterns.
pub fn detect_frameworks_ir(files: &[IrFile]) -> Vec<String> {
    let parsed: Vec<ParsedFile> = files.iter().map(|f| f.to_parsed_file()).collect();
    detect_frameworks(&parsed)
}

/// Build data flow edges directly from an IR file, without re-parsing source code.
///
/// This is the key improvement over the ParsedFile path: `IrFile` already contains
/// `assignments` (variable = call()) and `call_expressions` with arguments, so we
/// can trace producer → consumer edges without needing the original source text.
///
/// Example: given `const x = funcA(); funcB(x);` in the IR:
/// - `assignments` contains: pattern=x, value=Call(funcA), scope=f
/// - `call_expressions` contains: callee=funcB, args=["x"], scope=f
/// - Produces: `DataFlowEdge { producer: "funcA", consumer: "funcB", via: "x" }`
pub fn build_data_flow_edges_from_ir(file: &IrFile) -> Vec<DataFlowEdge> {
    let mut edges = Vec::new();

    // Group assignments by containing function for scope-aware matching.
    // Each entry: (variable_name, callee_name, line)
    let mut assignments_by_scope: HashMap<Option<&str>, Vec<(&str, &str, usize)>> =
        HashMap::new();

    for assignment in &file.assignments {
        if let (Some(var), Some(callee)) = (
            assignment.pattern.as_identifier(),
            assignment.value.callee_name(),
        ) {
            let scope = assignment.containing_function.as_deref();
            assignments_by_scope
                .entry(scope)
                .or_default()
                .push((var, callee, assignment.span.start_line));
        }
    }

    // For each call with arguments, check if any argument matches a variable
    // assigned from another call within the same scope.
    for call in &file.call_expressions {
        if call.arguments.is_empty() {
            continue;
        }
        let scope = call.containing_function.as_deref();
        if let Some(scope_assignments) = assignments_by_scope.get(&scope) {
            for arg in &call.arguments {
                for &(var, producer_callee, _line) in scope_assignments {
                    if var == arg.as_str() && producer_callee != call.callee {
                        let containing = match &call.containing_function {
                            Some(f) => format!("{}::{}", file.path, f),
                            None => file.path.to_string(),
                        };
                        edges.push(DataFlowEdge {
                            producer: producer_callee.to_string(),
                            consumer: call.callee.clone(),
                            via: arg.clone(),
                            containing_function: containing,
                            file: file.path.clone(),
                            line: call.span.start_line,
                        });
                    }
                }
            }
        }
    }

    edges
}

/// Trace data flow across all IR files, producing edges that show how data moves
/// through variable assignments and function calls.
///
/// Unlike `trace_data_flow` which requires source code and re-parses with tree-sitter,
/// this version works directly from the IR which already has assignments and call arguments.
pub fn trace_data_flow_ir(files: &[IrFile]) -> Vec<DataFlowEdge> {
    let mut all_edges = Vec::new();
    for file in files {
        let edges = build_data_flow_edges_from_ir(file);
        all_edges.extend(edges);
    }
    all_edges
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;
    use crate::ast::{self, ParsedFile};

    /// Helper: parse a file and detect heuristic patterns.
    fn detect_patterns(path: &str, source: &str) -> Vec<HeuristicEdge> {
        let parsed = ast::parse_file(path, source).unwrap();
        detect_heuristic_patterns(&parsed)
    }

    /// Helper: parse multiple files and run full flow analysis.
    fn analyze_files(files: &[(&str, &str)]) -> FlowAnalysis {
        let parsed: Vec<ParsedFile> = files
            .iter()
            .map(|(path, source)| ast::parse_file(path, source).unwrap())
            .collect();
        analyze_data_flow(&parsed, &FlowConfig::default())
    }

    /// Helper: detect frameworks from files.
    fn detect_fw(files: &[(&str, &str)]) -> Vec<String> {
        let parsed: Vec<ParsedFile> = files
            .iter()
            .map(|(path, source)| ast::parse_file(path, source).unwrap())
            .collect();
        detect_frameworks(&parsed)
    }

    /// Helper: build graph, enrich, and return it.
    fn build_and_enrich(files: &[(&str, &str)]) -> SymbolGraph {
        let parsed: Vec<ParsedFile> = files
            .iter()
            .map(|(path, source)| ast::parse_file(path, source).unwrap())
            .collect();
        let mut graph = SymbolGraph::build(&parsed);
        let analysis = analyze_data_flow(&parsed, &FlowConfig::default());
        enrich_graph(&mut graph, &analysis);
        graph
    }

    // ========================================================================
    // Heuristic: Database writes (Persistence)
    // ========================================================================

    #[test]
    fn test_heuristic_db_write_save() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function createUser(data: any) {
    return db.save(data);
}
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert_eq!(persistence.len(), 1);
        assert_eq!(persistence[0].evidence, "db.save");
        assert_eq!(persistence[0].from_symbol, "src/service.ts::createUser");
        assert!(persistence[0].confidence >= 0.7);
    }

    #[test]
    fn test_heuristic_db_write_insert() {
        let edges = detect_patterns(
            "src/repo.ts",
            r#"
function addItem(item: any) {
    return collection.insert(item);
}
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert_eq!(persistence.len(), 1);
        assert_eq!(persistence[0].evidence, "collection.insert");
    }

    #[test]
    fn test_heuristic_db_write_create() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function register(data: any) {
    return prisma.user.create(data);
}
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert!(
            !persistence.is_empty(),
            "should detect prisma.user.create as persistence"
        );
    }

    #[test]
    fn test_heuristic_db_write_delete() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function removeUser(id: string) {
    return userRepo.delete(id);
}
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert_eq!(persistence.len(), 1);
        assert_eq!(persistence[0].evidence, "userRepo.delete");
    }

    #[test]
    fn test_heuristic_db_write_update() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function updateProfile(id: string, data: any) {
    return db.update(data);
}
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert_eq!(persistence.len(), 1);
    }

    #[test]
    fn test_heuristic_db_write_python() {
        let edges = detect_patterns(
            "src/service.py",
            r#"
def create_user(data):
    session.flush()
    return session.execute(query)
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert!(persistence.len() >= 1, "should detect session.flush/execute");
    }

    #[test]
    fn test_heuristic_db_write_multiple_in_one_function() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function transferFunds(from: string, to: string, amount: number) {
    db.update({ id: from, balance: -amount });
    db.update({ id: to, balance: amount });
    auditStore.insert({ from, to, amount });
}
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert_eq!(
            persistence.len(),
            3,
            "should detect all 3 DB write calls in one function"
        );
    }

    // ========================================================================
    // Heuristic: Database reads
    // ========================================================================

    #[test]
    fn test_heuristic_db_read_find() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function getUser(id: string) {
    return userRepo.findOne(id);
}
"#,
        );

        let reads: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::DatabaseRead)
            .collect();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].evidence, "userRepo.findOne");
    }

    #[test]
    fn test_heuristic_db_read_query() {
        let edges = detect_patterns(
            "src/repo.ts",
            r#"
function listUsers() {
    return db.query("SELECT * FROM users");
}
"#,
        );

        let reads: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::DatabaseRead)
            .collect();
        assert!(
            !reads.is_empty(),
            "should detect db.query as database read"
        );
    }

    #[test]
    fn test_heuristic_db_read_find_many() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function getActiveUsers() {
    return prisma.user.findMany({ where: { active: true } });
}
"#,
        );

        let reads: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::DatabaseRead)
            .collect();
        assert!(!reads.is_empty());
    }

    // ========================================================================
    // Heuristic: Event emission
    // ========================================================================

    #[test]
    fn test_heuristic_event_emission() {
        let edges = detect_patterns(
            "src/handler.ts",
            r#"
function onUserCreated(user: any) {
    eventBus.emit('user:created', user);
}
"#,
        );

        let emissions: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::EventEmission)
            .collect();
        assert_eq!(emissions.len(), 1);
        assert_eq!(emissions[0].evidence, "eventBus.emit");
    }

    #[test]
    fn test_heuristic_event_publish() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function notifySubscribers(data: any) {
    queue.publish(data);
}
"#,
        );

        let emissions: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::EventEmission)
            .collect();
        assert_eq!(emissions.len(), 1);
        assert_eq!(emissions[0].evidence, "queue.publish");
    }

    #[test]
    fn test_heuristic_event_dispatch() {
        let edges = detect_patterns(
            "src/handler.ts",
            r#"
function handleClick() {
    store.dispatch({ type: 'CLICK' });
}
"#,
        );

        let emissions: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::EventEmission)
            .collect();
        assert_eq!(emissions.len(), 1);
    }

    #[test]
    fn test_heuristic_event_send() {
        let edges = detect_patterns(
            "src/worker.ts",
            r#"
function processJob(job: any) {
    channel.send(result);
}
"#,
        );

        let emissions: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::EventEmission)
            .collect();
        assert_eq!(emissions.len(), 1);
    }

    // ========================================================================
    // Heuristic: Event handling
    // ========================================================================

    #[test]
    fn test_heuristic_event_handling_on() {
        let edges = detect_patterns(
            "src/listener.ts",
            r#"
function setupListeners() {
    emitter.on('data', handleData);
}
"#,
        );

        let handlers: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::EventHandling)
            .collect();
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].evidence, "emitter.on");
    }

    #[test]
    fn test_heuristic_event_subscribe() {
        let edges = detect_patterns(
            "src/consumer.ts",
            r#"
function init() {
    topic.subscribe(processMessage);
}
"#,
        );

        let handlers: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::EventHandling)
            .collect();
        assert_eq!(handlers.len(), 1);
    }

    #[test]
    fn test_heuristic_event_listen() {
        let edges = detect_patterns(
            "src/server.ts",
            r#"
function start() {
    server.listen(3000);
}
"#,
        );

        let handlers: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::EventHandling)
            .collect();
        assert_eq!(handlers.len(), 1);
    }

    // ========================================================================
    // Heuristic: Config reads
    // ========================================================================

    #[test]
    fn test_heuristic_config_read_process_env() {
        // Note: `process.env.DATABASE_URL` is a member access, not a function call,
        // so tree-sitter won't capture it as a call site. We test with a function
        // call pattern like `process.env.get()` or via direct classifier.
        let call = crate::ast::CallSite {
            callee: "process.env.DATABASE_URL".to_string(),
            line: 3,
            containing_function: Some("getDbUrl".to_string()),
        };
        let edge = classify_call_site(&call, "src/config.ts");
        assert!(edge.is_some(), "should detect process.env as config read");
        assert_eq!(edge.unwrap().pattern, FlowPattern::ConfigRead);
    }

    #[test]
    fn test_heuristic_config_read_python_environ() {
        let edges = detect_patterns(
            "src/config.py",
            r#"
def get_db_url():
    return os.environ.get("DATABASE_URL")
"#,
        );

        // os.environ should be detected (os.environ matches the prefix)
        let configs: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::ConfigRead)
            .collect();
        assert!(!configs.is_empty(), "should detect os.environ as config read");
    }

    #[test]
    fn test_heuristic_config_read_python_getenv() {
        let edges = detect_patterns(
            "src/config.py",
            r#"
def get_secret():
    return os.getenv("SECRET_KEY")
"#,
        );

        let configs: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::ConfigRead)
            .collect();
        assert!(!configs.is_empty());
    }

    // ========================================================================
    // Heuristic: HTTP calls
    // ========================================================================

    #[test]
    fn test_heuristic_http_call_fetch() {
        let edges = detect_patterns(
            "src/client.ts",
            r#"
function getData() {
    return fetch('/api/data');
}
"#,
        );

        let http: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::HttpCall)
            .collect();
        assert_eq!(http.len(), 1);
        assert_eq!(http[0].evidence, "fetch");
    }

    #[test]
    fn test_heuristic_http_call_axios() {
        let edges = detect_patterns(
            "src/api.ts",
            r#"
function fetchUsers() {
    return axios.get('/users');
}
"#,
        );

        let http: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::HttpCall)
            .collect();
        assert_eq!(http.len(), 1);
    }

    #[test]
    fn test_heuristic_http_call_python_requests() {
        let edges = detect_patterns(
            "src/client.py",
            r#"
def get_data():
    return requests.get("https://api.example.com/data")
"#,
        );

        let http: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::HttpCall)
            .collect();
        assert_eq!(http.len(), 1);
    }

    // ========================================================================
    // Heuristic: Logging
    // ========================================================================

    #[test]
    fn test_heuristic_logging_console() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function process(data: any) {
    console.log('processing', data);
    console.error('failed');
}
"#,
        );

        let logs: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Logging)
            .collect();
        assert_eq!(logs.len(), 2);
    }

    #[test]
    fn test_heuristic_logging_logger() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function handleRequest(req: any) {
    logger.info('handling request');
    logger.error('failed');
}
"#,
        );

        let logs: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Logging)
            .collect();
        assert_eq!(logs.len(), 2);
    }

    #[test]
    fn test_heuristic_logging_python() {
        let edges = detect_patterns(
            "src/handler.py",
            r#"
def process(data):
    logging.info("processing")
    logging.error("failed")
"#,
        );

        let logs: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Logging)
            .collect();
        assert_eq!(logs.len(), 2);
    }

    // ========================================================================
    // False positive guards
    // ========================================================================

    #[test]
    fn test_no_false_positive_array_methods() {
        let edges = detect_patterns(
            "src/utils.ts",
            r#"
function process(items: any[]) {
    const found = items.find(x => x.id === 1);
    items.push(newItem);
    items.filter(x => x.active);
    items.map(x => x.name);
    items.forEach(x => handle(x));
    items.includes(target);
    items.sort();
    items.reverse();
}
"#,
        );

        let db_patterns: Vec<_> = edges
            .iter()
            .filter(|e| {
                e.pattern == FlowPattern::Persistence || e.pattern == FlowPattern::DatabaseRead
            })
            .collect();
        assert!(
            db_patterns.is_empty(),
            "array methods should not trigger DB patterns, got: {:?}",
            db_patterns
        );
    }

    #[test]
    fn test_no_false_positive_map_set_methods() {
        let edges = detect_patterns(
            "src/utils.ts",
            r#"
function process() {
    const m = new Map();
    m.get('key');
    m.set('key', 'value');
    m.delete('key');
    m.has('key');
    const s = new Set();
    s.add(1);
    s.delete(1);
    s.has(1);
}
"#,
        );

        // These should be filtered out as common collection operations
        let db_patterns: Vec<_> = edges
            .iter()
            .filter(|e| {
                e.pattern == FlowPattern::Persistence || e.pattern == FlowPattern::DatabaseRead
            })
            .collect();
        assert!(
            db_patterns.is_empty(),
            "Map/Set methods should not trigger DB patterns, got: {:?}",
            db_patterns
        );
    }

    #[test]
    fn test_no_false_positive_console_as_db() {
        let edges = detect_patterns(
            "src/utils.ts",
            r#"
function debug(msg: string) {
    console.log(msg);
}
"#,
        );

        // console.log should be Logging, not Persistence/DatabaseRead
        let db_patterns: Vec<_> = edges
            .iter()
            .filter(|e| {
                e.pattern == FlowPattern::Persistence || e.pattern == FlowPattern::DatabaseRead
            })
            .collect();
        assert!(db_patterns.is_empty());

        let logs: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Logging)
            .collect();
        assert_eq!(logs.len(), 1);
    }

    #[test]
    fn test_no_false_positive_json_parse() {
        let edges = detect_patterns(
            "src/utils.ts",
            r#"
function parse(data: string) {
    return JSON.parse(data);
}
"#,
        );

        let db_patterns: Vec<_> = edges
            .iter()
            .filter(|e| {
                e.pattern == FlowPattern::Persistence || e.pattern == FlowPattern::DatabaseRead
            })
            .collect();
        assert!(db_patterns.is_empty());
    }

    #[test]
    fn test_no_false_positive_promise_resolve() {
        let edges = detect_patterns(
            "src/utils.ts",
            r#"
function wrap(value: any) {
    return Promise.resolve(value);
}
"#,
        );

        // Should not be detected as any pattern (Promise is filtered)
        assert!(edges.is_empty(), "Promise.resolve should not trigger any pattern");
    }

    #[test]
    fn test_no_false_positive_localstorage() {
        let edges = detect_patterns(
            "src/store.ts",
            r#"
function savePreference(key: string, value: string) {
    localStorage.set(key, value);
    localStorage.get(key);
}
"#,
        );

        // localStorage operations should be filtered
        let db_patterns: Vec<_> = edges
            .iter()
            .filter(|e| {
                e.pattern == FlowPattern::Persistence || e.pattern == FlowPattern::DatabaseRead
            })
            .collect();
        assert!(
            db_patterns.is_empty(),
            "localStorage should not trigger DB patterns"
        );
    }

    // ========================================================================
    // Framework detection
    // ========================================================================

    #[test]
    fn test_detect_express() {
        let frameworks = detect_fw(&[(
            "src/app.ts",
            r#"
import express from 'express';
const app = express();
"#,
        )]);
        assert!(frameworks.contains(&"Express".to_string()));
    }

    #[test]
    fn test_detect_nextjs_from_imports() {
        let frameworks = detect_fw(&[(
            "src/page.tsx",
            r#"
import { useRouter } from 'next/router';
import Head from 'next/head';
"#,
        )]);
        assert!(frameworks.contains(&"Next.js".to_string()));
    }

    #[test]
    fn test_detect_nextjs_from_file_structure() {
        let parsed = vec![ParsedFile {
            path: "app/dashboard/page.tsx".to_string(),
            language: crate::ast::Language::TypeScript,
            definitions: vec![],
            imports: vec![],
            exports: vec![],
            call_sites: vec![],
        }];
        let frameworks = detect_frameworks(&parsed);
        assert!(frameworks.contains(&"Next.js".to_string()));
    }

    #[test]
    fn test_detect_react() {
        let frameworks = detect_fw(&[(
            "src/App.tsx",
            r#"
import React from 'react';
import { useState } from 'react';
"#,
        )]);
        assert!(frameworks.contains(&"React".to_string()));
    }

    #[test]
    fn test_detect_fastapi() {
        let frameworks = detect_fw(&[(
            "src/main.py",
            r#"
from fastapi import FastAPI
app = FastAPI()
"#,
        )]);
        assert!(frameworks.contains(&"FastAPI".to_string()));
    }

    #[test]
    fn test_detect_flask() {
        let frameworks = detect_fw(&[(
            "src/app.py",
            r#"
from flask import Flask
app = Flask(__name__)
"#,
        )]);
        assert!(frameworks.contains(&"Flask".to_string()));
    }

    #[test]
    fn test_detect_django() {
        let frameworks = detect_fw(&[(
            "src/views.py",
            r#"
from django.http import HttpResponse
from django.views import View
"#,
        )]);
        assert!(frameworks.contains(&"Django".to_string()));
    }

    #[test]
    fn test_detect_prisma() {
        let frameworks = detect_fw(&[(
            "src/db.ts",
            r#"
import { PrismaClient } from '@prisma/client';
const prisma = new PrismaClient();
"#,
        )]);
        assert!(frameworks.contains(&"Prisma".to_string()));
    }

    #[test]
    fn test_detect_effect_ts() {
        let frameworks = detect_fw(&[(
            "src/service.ts",
            r#"
import { Effect } from 'effect';
import { HttpApi } from '@effect/platform';
"#,
        )]);
        assert!(frameworks.contains(&"Effect.ts".to_string()));
    }

    #[test]
    fn test_detect_multiple_frameworks() {
        let frameworks = detect_fw(&[
            (
                "src/app.ts",
                r#"
import express from 'express';
import { PrismaClient } from '@prisma/client';
"#,
            ),
            (
                "src/App.tsx",
                r#"
import React from 'react';
"#,
            ),
        ]);
        assert!(frameworks.contains(&"Express".to_string()));
        assert!(frameworks.contains(&"Prisma".to_string()));
        assert!(frameworks.contains(&"React".to_string()));
    }

    #[test]
    fn test_detect_no_frameworks() {
        let frameworks = detect_fw(&[(
            "src/utils.ts",
            r#"
function add(a: number, b: number) { return a + b; }
"#,
        )]);
        assert!(frameworks.is_empty());
    }

    #[test]
    fn test_detect_frameworks_sorted() {
        let frameworks = detect_fw(&[(
            "src/app.ts",
            r#"
import { z } from 'zod';
import express from 'express';
import { PrismaClient } from '@prisma/client';
"#,
        )]);
        // Should be sorted alphabetically
        let is_sorted = frameworks.windows(2).all(|w| w[0] <= w[1]);
        assert!(is_sorted, "frameworks should be sorted: {:?}", frameworks);
    }

    #[test]
    fn test_detect_frameworks_deduplicated() {
        let frameworks = detect_fw(&[
            (
                "src/a.ts",
                r#"
import React from 'react';
"#,
            ),
            (
                "src/b.ts",
                r#"
import { useState } from 'react';
import { useEffect } from 'react';
"#,
            ),
        ]);
        let react_count = frameworks.iter().filter(|f| *f == "React").count();
        assert_eq!(react_count, 1, "React should appear exactly once");
    }

    // ========================================================================
    // Go framework detection
    // ========================================================================

    #[test]
    fn test_detect_go_gin_framework() {
        let frameworks = detect_fw(&[(
            "main.go",
            r#"
package main

import "github.com/gin-gonic/gin"

func main() {
    r := gin.Default()
    r.Run(":8080")
}
"#,
        )]);
        assert!(
            frameworks.contains(&"Gin".to_string()),
            "should detect Gin framework, got: {:?}",
            frameworks,
        );
    }

    #[test]
    fn test_detect_go_multiple_frameworks() {
        let frameworks = detect_fw(&[
            (
                "main.go",
                r#"
package main

import (
    "net/http"
    "github.com/spf13/cobra"
    "gorm.io/gorm"
)

func main() {
    http.ListenAndServe(":8080", nil)
}
"#,
            ),
        ]);
        assert!(frameworks.contains(&"Go net/http".to_string()));
        assert!(frameworks.contains(&"Cobra".to_string()));
        assert!(frameworks.contains(&"GORM".to_string()));
    }

    #[test]
    fn test_detect_go_grpc_framework() {
        let frameworks = detect_fw(&[(
            "server.go",
            r#"
package main

import "google.golang.org/grpc"

func main() {
    s := grpc.NewServer()
}
"#,
        )]);
        assert!(
            frameworks.contains(&"gRPC".to_string()),
            "should detect gRPC framework, got: {:?}",
            frameworks,
        );
    }

    // ========================================================================
    // Full flow analysis integration
    // ========================================================================

    #[test]
    fn test_analyze_full_flow_mixed_patterns() {
        let analysis = analyze_files(&[(
            "src/handler.ts",
            r#"
import { validate } from './validator';

function handleCreateUser(req: any) {
    console.log('handling request');
    const data = validate(req.body);
    const user = db.save(data);
    eventBus.emit('user:created', user);
    return user;
}
"#,
        )]);

        let patterns: Vec<&FlowPattern> = analysis
            .heuristic_edges
            .iter()
            .map(|e| &e.pattern)
            .collect();

        assert!(
            patterns.contains(&&FlowPattern::Logging),
            "should detect console.log"
        );
        assert!(
            patterns.contains(&&FlowPattern::Persistence),
            "should detect db.save"
        );
        assert!(
            patterns.contains(&&FlowPattern::EventEmission),
            "should detect eventBus.emit"
        );
    }

    #[test]
    fn test_analyze_empty_files() {
        let analysis = analyze_files(&[]);
        assert!(analysis.heuristic_edges.is_empty());
        assert!(analysis.frameworks_detected.is_empty());
    }

    #[test]
    fn test_analyze_no_patterns() {
        let analysis = analyze_files(&[(
            "src/utils.ts",
            r#"
function add(a: number, b: number): number {
    return a + b;
}
"#,
        )]);
        assert!(analysis.heuristic_edges.is_empty());
    }

    // ========================================================================
    // Graph enrichment
    // ========================================================================

    #[test]
    fn test_enrich_graph_adds_writes_edge() {
        let graph = build_and_enrich(&[
            (
                "src/db.ts",
                r#"
export function save(data: any) { return data; }
"#,
            ),
            (
                "src/service.ts",
                r#"
import { save } from './db';
function createUser(data: any) {
    return db.save(data);
}
"#,
            ),
        ]);

        let writes_edges: Vec<_> = graph
            .edges()
            .into_iter()
            .filter(|(_, _, et)| **et == EdgeType::Writes)
            .collect();

        // There should be a Writes edge from createUser (or the service module)
        assert!(
            !writes_edges.is_empty(),
            "should have Writes edge from heuristic inference"
        );
    }

    #[test]
    fn test_enrich_graph_adds_emits_edge() {
        let graph = build_and_enrich(&[(
            "src/handler.ts",
            r#"
function notify(data: any) {
    eventBus.emit('event', data);
}
"#,
        )]);

        let emits_edges: Vec<_> = graph
            .edges()
            .into_iter()
            .filter(|(_, _, et)| **et == EdgeType::Emits)
            .collect();

        // Emits edge should exist (from notify to module node)
        // Note: won't exist if from_symbol == module node (self-edge guard)
        // The function node should have an Emits edge
        assert!(
            !emits_edges.is_empty(),
            "should have Emits edge from heuristic inference"
        );
    }

    #[test]
    fn test_enrich_graph_no_logging_edges() {
        let graph = build_and_enrich(&[(
            "src/service.ts",
            r#"
function process(data: any) {
    console.log('processing');
    return data;
}
"#,
        )]);

        // Logging should NOT create graph edges
        let all_edges = graph.edges();
        let logging_related: Vec<_> = all_edges
            .iter()
            .filter(|(f, _, _)| f.contains("process"))
            .collect();
        // process should have no outgoing edges (console.log is filtered from graph)
        assert!(
            logging_related.is_empty(),
            "logging should not create graph edges"
        );
    }

    // ========================================================================
    // Call chain tracing
    // ========================================================================

    #[test]
    fn test_trace_call_chain_simple() {
        let parsed: Vec<ParsedFile> = [
            (
                "src/a.ts",
                r#"
import { funcB } from './b';
export function funcA() { funcB(); }
"#,
            ),
            (
                "src/b.ts",
                r#"
import { funcC } from './c';
export function funcB() { funcC(); }
"#,
            ),
            (
                "src/c.ts",
                r#"
export function funcC() { return 42; }
"#,
            ),
        ]
        .iter()
        .map(|(path, source)| ast::parse_file(path, source).unwrap())
        .collect();

        let graph = SymbolGraph::build(&parsed);
        let chain = trace_call_chain(&graph, "src/a.ts::funcA", 10);

        assert!(
            chain.contains(&"src/b.ts::funcB".to_string()),
            "should reach funcB from funcA"
        );
        assert!(
            chain.contains(&"src/c.ts::funcC".to_string()),
            "should reach funcC from funcA through funcB"
        );
    }

    #[test]
    fn test_trace_call_chain_depth_limit() {
        let parsed: Vec<ParsedFile> = [
            (
                "src/a.ts",
                r#"
import { funcB } from './b';
export function funcA() { funcB(); }
"#,
            ),
            (
                "src/b.ts",
                r#"
import { funcC } from './c';
export function funcB() { funcC(); }
"#,
            ),
            (
                "src/c.ts",
                r#"
export function funcC() { return 42; }
"#,
            ),
        ]
        .iter()
        .map(|(path, source)| ast::parse_file(path, source).unwrap())
        .collect();

        let graph = SymbolGraph::build(&parsed);

        // Depth 1 should only reach direct callees
        let chain = trace_call_chain(&graph, "src/a.ts::funcA", 1);
        assert!(
            chain.contains(&"src/b.ts::funcB".to_string()),
            "depth 1 should reach funcB"
        );
        assert!(
            !chain.contains(&"src/c.ts::funcC".to_string()),
            "depth 1 should NOT reach funcC"
        );
    }

    #[test]
    fn test_trace_call_chain_nonexistent_start() {
        let parsed: Vec<ParsedFile> = [(
            "src/a.ts",
            r#"
export function funcA() { return 1; }
"#,
        )]
        .iter()
        .map(|(path, source)| ast::parse_file(path, source).unwrap())
        .collect();

        let graph = SymbolGraph::build(&parsed);
        let chain = trace_call_chain(&graph, "nonexistent::func", 10);
        assert!(chain.is_empty());
    }

    #[test]
    fn test_trace_call_chain_no_calls() {
        let parsed: Vec<ParsedFile> = [(
            "src/a.ts",
            r#"
export function funcA() { return 1; }
"#,
        )]
        .iter()
        .map(|(path, source)| ast::parse_file(path, source).unwrap())
        .collect();

        let graph = SymbolGraph::build(&parsed);
        let chain = trace_call_chain(&graph, "src/a.ts::funcA", 10);
        assert!(chain.is_empty());
    }

    #[test]
    fn test_trace_call_chain_cyclic() {
        let parsed: Vec<ParsedFile> = [
            (
                "src/a.ts",
                r#"
import { funcB } from './b';
export function funcA() { funcB(); }
"#,
            ),
            (
                "src/b.ts",
                r#"
import { funcA } from './a';
export function funcB() { funcA(); }
"#,
            ),
        ]
        .iter()
        .map(|(path, source)| ast::parse_file(path, source).unwrap())
        .collect();

        let graph = SymbolGraph::build(&parsed);
        // Should not infinite loop
        let chain = trace_call_chain(&graph, "src/a.ts::funcA", 10);
        assert!(
            chain.contains(&"src/b.ts::funcB".to_string()),
            "should reach funcB despite cycle"
        );
        // funcA should not appear in chain (it's the start)
        // funcB should appear exactly once
        let funcb_count = chain.iter().filter(|s| *s == "src/b.ts::funcB").count();
        assert_eq!(funcb_count, 1, "funcB should appear exactly once");
    }

    // ========================================================================
    // Confidence scoring
    // ========================================================================

    #[test]
    fn test_confidence_orm_specific_high() {
        let c = confidence_for_db_pattern("prisma.user.create");
        assert!(c >= 0.9, "ORM-specific patterns should have high confidence");
    }

    #[test]
    fn test_confidence_db_receiver_high() {
        let c = confidence_for_db_pattern("db.save");
        assert!(c >= 0.9, "db.* patterns should have high confidence");
    }

    #[test]
    fn test_confidence_repo_receiver_high() {
        let c = confidence_for_db_pattern("userRepo.save");
        assert!(
            c >= 0.9,
            "repo.* patterns should have high confidence"
        );
    }

    #[test]
    fn test_confidence_generic_medium() {
        let c = confidence_for_db_pattern("service.save");
        assert!(
            c < 0.9 && c >= 0.5,
            "generic patterns should have medium confidence: {}",
            c
        );
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn test_module_level_call() {
        let edges = detect_patterns(
            "src/setup.ts",
            r#"
db.save({ init: true });
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert_eq!(persistence.len(), 1);
        // Module-level calls use the file path as from_symbol
        assert_eq!(persistence[0].from_symbol, "src/setup.ts");
    }

    #[test]
    fn test_line_numbers_preserved() {
        let edges = detect_patterns(
            "src/service.ts",
            r#"
function handler() {
    console.log('start');
    db.save(data);
    eventBus.emit('done');
}
"#,
        );

        assert!(!edges.is_empty());
        // All edges should have valid line numbers
        for edge in &edges {
            assert!(edge.line > 0, "line number should be positive");
        }
    }

    #[test]
    fn test_pattern_in_nested_function() {
        let edges = detect_patterns(
            "src/handler.ts",
            r#"
function outer() {
    function inner() {
        db.save(data);
    }
}
"#,
        );

        let persistence: Vec<_> = edges
            .iter()
            .filter(|e| e.pattern == FlowPattern::Persistence)
            .collect();
        assert_eq!(persistence.len(), 1);
        // Should attribute to the innermost containing function
        assert!(persistence[0].from_symbol.contains("inner"));
    }

    // ========================================================================
    // Property-based tests
    // ========================================================================

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Generate a random callee string.
        fn callee_strategy() -> impl Strategy<Value = String> {
            prop_oneof![
                // Normal function calls
                "[a-z][a-zA-Z0-9]{0,15}".prop_map(|s| s),
                // Method calls
                "[a-z][a-zA-Z0-9]{0,10}\\.[a-z][a-zA-Z0-9]{0,10}".prop_map(|s| s),
                // Known DB methods
                Just("db.save".to_string()),
                Just("db.find".to_string()),
                Just("db.delete".to_string()),
                // Known event methods
                Just("emitter.emit".to_string()),
                Just("bus.publish".to_string()),
                // Known config
                Just("process.env".to_string()),
                // Known logging
                Just("console.log".to_string()),
                Just("logger.info".to_string()),
                // Known HTTP
                Just("fetch".to_string()),
                Just("axios.get".to_string()),
            ]
        }

        proptest! {
            #[test]
            fn prop_classify_never_panics(callee in callee_strategy()) {
                let call = CallSite {
                    callee,
                    line: 1,
                    containing_function: Some("testFunc".to_string()),
                };
                // Should never panic regardless of input
                let _ = classify_call_site(&call, "test.ts");
            }

            #[test]
            fn prop_confidence_in_range(callee in "[a-z][a-zA-Z0-9.]{0,30}") {
                let c = confidence_for_db_pattern(&callee);
                prop_assert!(c >= 0.0 && c <= 1.0,
                    "confidence {} for '{}' should be in [0.0, 1.0]", c, callee);
            }

            #[test]
            fn prop_heuristic_edge_has_valid_fields(
                func_name in "[a-z][a-zA-Z0-9]{0,10}",
                callee in callee_strategy()
            ) {
                let call = CallSite {
                    callee: callee.clone(),
                    line: 42,
                    containing_function: Some(func_name),
                };
                if let Some(edge) = classify_call_site(&call, "src/test.ts") {
                    prop_assert!(!edge.from_symbol.is_empty());
                    prop_assert!(!edge.file.is_empty());
                    prop_assert!(!edge.evidence.is_empty());
                    prop_assert!(edge.confidence > 0.0 && edge.confidence <= 1.0);
                    prop_assert!(edge.line == 42);
                }
            }

            #[test]
            fn prop_frameworks_always_sorted(
                imports in prop::collection::vec(
                    prop_oneof![
                        Just("react"),
                        Just("express"),
                        Just("next/router"),
                        Just("vue"),
                        Just("fastapi"),
                        Just("django"),
                        Just("./local"),
                        Just("unknown-pkg"),
                    ],
                    0..10
                )
            ) {
                let files: Vec<ParsedFile> = imports.iter().enumerate().map(|(i, imp)| {
                    ParsedFile {
                        path: format!("src/file{}.ts", i),
                        language: crate::ast::Language::TypeScript,
                        definitions: vec![],
                        imports: vec![crate::ast::ImportInfo {
                            source: imp.to_string(),
                            names: vec![],
                            is_default: false,
                            is_namespace: false,
                            line: 1,
                        }],
                        exports: vec![],
                        call_sites: vec![],
                    }
                }).collect();

                let frameworks = detect_frameworks(&files);
                let is_sorted = frameworks.windows(2).all(|w| w[0] <= w[1]);
                prop_assert!(is_sorted, "frameworks should be sorted: {:?}", frameworks);

                // No duplicates
                let unique: HashSet<_> = frameworks.iter().collect();
                prop_assert_eq!(unique.len(), frameworks.len(),
                    "frameworks should have no duplicates: {:?}", frameworks);
            }

            #[test]
            fn prop_analyze_empty_input_empty_output(_dummy in 0u32..1) {
                let analysis = analyze_data_flow(&[], &FlowConfig::default());
                prop_assert!(analysis.heuristic_edges.is_empty());
                prop_assert!(analysis.frameworks_detected.is_empty());
            }

            #[test]
            fn prop_deterministic_analysis(callee in callee_strategy()) {
                let file = ParsedFile {
                    path: "src/test.ts".to_string(),
                    language: crate::ast::Language::TypeScript,
                    definitions: vec![],
                    imports: vec![],
                    exports: vec![],
                    call_sites: vec![CallSite {
                        callee,
                        line: 1,
                        containing_function: Some("test".to_string()),
                    }],
                };

                let a1 = analyze_data_flow(&[file.clone()], &FlowConfig::default());
                let a2 = analyze_data_flow(&[file], &FlowConfig::default());
                prop_assert_eq!(a1.heuristic_edges.len(), a2.heuristic_edges.len());
                prop_assert_eq!(a1.frameworks_detected, a2.frameworks_detected);
            }
        }
    }

    // ========================================================================
    // Full data flow tracing — unit tests
    // ========================================================================

    #[test]
    fn test_data_flow_simple_variable_chain() {
        let edges = trace_data_flow(&[(
            "src/handler.ts",
            r#"
function handler(req: any) {
    const data = parseBody(req);
    return respond(data);
}
"#,
        )]);

        // parseBody produces `data`, respond consumes `data`
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].producer, "parseBody");
        assert_eq!(edges[0].consumer, "respond");
        assert_eq!(edges[0].via, "data");
        assert_eq!(edges[0].containing_function, "src/handler.ts::handler");
        assert_eq!(edges[0].file, "src/handler.ts");
    }

    #[test]
    fn test_data_flow_chained_pipeline() {
        let edges = trace_data_flow(&[(
            "src/pipeline.ts",
            r#"
function pipeline(input: any) {
    const validated = validate(input);
    const processed = transform(validated);
    const result = save(processed);
    return result;
}
"#,
        )]);

        // validate → transform via "validated"
        let e1 = edges
            .iter()
            .find(|e| e.producer == "validate" && e.consumer == "transform")
            .expect("should have validate → transform edge");
        assert_eq!(e1.via, "validated");

        // transform → save via "processed"
        let e2 = edges
            .iter()
            .find(|e| e.producer == "transform" && e.consumer == "save")
            .expect("should have transform → save edge");
        assert_eq!(e2.via, "processed");

        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn test_data_flow_multiple_consumers() {
        let edges = trace_data_flow(&[(
            "src/process.ts",
            r#"
function process() {
    const data = fetchData();
    validate(data);
    transform(data);
    save(data);
}
"#,
        )]);

        // fetchData → validate, transform, save (all via "data")
        assert_eq!(edges.len(), 3);
        for edge in &edges {
            assert_eq!(edge.producer, "fetchData");
            assert_eq!(edge.via, "data");
        }
        let consumers: Vec<&str> = edges.iter().map(|e| e.consumer.as_str()).collect();
        assert!(consumers.contains(&"validate"));
        assert!(consumers.contains(&"transform"));
        assert!(consumers.contains(&"save"));
    }

    #[test]
    fn test_data_flow_no_variable_sharing() {
        let edges = trace_data_flow(&[(
            "src/independent.ts",
            r#"
function handler() {
    funcA();
    funcB();
    funcC();
}
"#,
        )]);

        assert!(
            edges.is_empty(),
            "independent calls with no shared variables should produce no edges"
        );
    }

    #[test]
    fn test_data_flow_module_level() {
        let edges = trace_data_flow(&[(
            "src/main.ts",
            r#"
const config = loadConfig();
startServer(config);
"#,
        )]);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].producer, "loadConfig");
        assert_eq!(edges[0].consumer, "startServer");
        assert_eq!(edges[0].via, "config");
        assert_eq!(edges[0].containing_function, "src/main.ts");
    }

    #[test]
    fn test_data_flow_python_simple() {
        let edges = trace_data_flow(&[(
            "src/handler.py",
            r#"
def handler(req):
    data = parse_body(req)
    return respond(data)
"#,
        )]);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].producer, "parse_body");
        assert_eq!(edges[0].consumer, "respond");
        assert_eq!(edges[0].via, "data");
    }

    #[test]
    fn test_data_flow_python_chained() {
        let edges = trace_data_flow(&[(
            "src/pipeline.py",
            r#"
def pipeline(raw):
    validated = validate(raw)
    processed = transform(validated)
    save(processed)
"#,
        )]);

        let e1 = edges
            .iter()
            .find(|e| e.producer == "validate" && e.consumer == "transform");
        assert!(e1.is_some(), "should have validate → transform edge");

        let e2 = edges
            .iter()
            .find(|e| e.producer == "transform" && e.consumer == "save");
        assert!(e2.is_some(), "should have transform → save edge");
    }

    #[test]
    fn test_data_flow_empty_input() {
        let edges = trace_data_flow(&[]);
        assert!(edges.is_empty());
    }

    #[test]
    fn test_data_flow_unknown_language() {
        let edges = trace_data_flow(&[("main.go", "package main")]);
        assert!(edges.is_empty());
    }

    #[test]
    fn test_data_flow_cross_scope_no_leaking() {
        let edges = trace_data_flow(&[(
            "src/service.ts",
            r#"
function funcA() {
    const x = getX();
    return x;
}
function funcB() {
    useX(x);
}
"#,
        )]);

        // `x` in funcA is not the same scope as `x` in funcB
        // funcA has assignment x=getX() but funcB's call useX(x)
        // is in a different scope, so no data flow edge should be created.
        assert!(
            edges.is_empty(),
            "variables in different function scopes should not create edges"
        );
    }

    #[test]
    fn test_data_flow_same_producer_consumer_ignored() {
        let edges = trace_data_flow(&[(
            "src/recursive.ts",
            r#"
function process() {
    const result = transform(input);
    transform(result);
}
"#,
        )]);

        // transform → transform would be a self-edge, should be excluded
        assert!(
            edges.is_empty(),
            "same producer and consumer should not create an edge"
        );
    }

    #[test]
    fn test_data_flow_multiple_files() {
        let edges = trace_data_flow(&[
            (
                "src/a.ts",
                r#"
function handleA() {
    const x = getA();
    processA(x);
}
"#,
            ),
            (
                "src/b.ts",
                r#"
function handleB() {
    const y = getB();
    processB(y);
}
"#,
            ),
        ]);

        assert_eq!(edges.len(), 2);
        assert!(edges.iter().any(|e| e.file == "src/a.ts"));
        assert!(edges.iter().any(|e| e.file == "src/b.ts"));
    }

    #[test]
    fn test_data_flow_deterministic() {
        let source = r#"
function handler() {
    const data = fetch();
    process(data);
}
"#;
        let e1 = trace_data_flow(&[("src/a.ts", source)]);
        let e2 = trace_data_flow(&[("src/a.ts", source)]);
        assert_eq!(e1, e2, "data flow tracing should be deterministic");
    }

    // ========================================================================
    // Full data flow tracing — property-based tests
    // ========================================================================

    mod data_flow_proptests {
        use super::*;
        use proptest::prelude::*;

        fn func_name() -> impl Strategy<Value = String> {
            "[a-z][a-zA-Z]{0,8}".prop_map(|s| s)
        }

        proptest! {
            #[test]
            fn prop_trace_never_panics(
                func in func_name(),
                var in func_name(),
                callee1 in func_name(),
                callee2 in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee1, callee2, var
                );
                // Should never panic
                let _ = trace_data_flow(&[("test.ts", &source)]);
            }

            #[test]
            fn prop_edges_have_valid_fields(
                func in func_name(),
                var in func_name(),
                callee1 in func_name(),
                callee2 in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee1, callee2, var
                );
                let edges = trace_data_flow(&[("test.ts", &source)]);
                for edge in &edges {
                    prop_assert!(!edge.producer.is_empty(), "producer should not be empty");
                    prop_assert!(!edge.consumer.is_empty(), "consumer should not be empty");
                    prop_assert!(!edge.via.is_empty(), "via should not be empty");
                    prop_assert!(!edge.file.is_empty(), "file should not be empty");
                    prop_assert!(edge.line > 0, "line should be positive");
                    prop_assert!(!edge.containing_function.is_empty());
                }
            }

            #[test]
            fn prop_no_self_edges(
                func in func_name(),
                var in func_name(),
                callee in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee, callee, var
                );
                let edges = trace_data_flow(&[("test.ts", &source)]);
                for edge in &edges {
                    prop_assert!(
                        edge.producer != edge.consumer,
                        "self-edge detected: {} → {}",
                        edge.producer, edge.consumer
                    );
                }
            }

            #[test]
            fn prop_via_matches_variable(
                func in func_name(),
                var in func_name(),
                callee1 in func_name(),
                callee2 in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee1, callee2, var
                );
                let edges = trace_data_flow(&[("test.ts", &source)]);
                for edge in &edges {
                    // The via field should be a variable that was assigned in the same scope
                    prop_assert_eq!(&edge.via, &var,
                        "via should match the variable name");
                }
            }

            #[test]
            fn prop_deterministic(
                func in func_name(),
                var in func_name(),
                callee1 in func_name(),
                callee2 in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee1, callee2, var
                );
                let e1 = trace_data_flow(&[("test.ts", &source)]);
                let e2 = trace_data_flow(&[("test.ts", &source)]);
                prop_assert_eq!(e1, e2, "should be deterministic");
            }

            #[test]
            fn prop_empty_input_empty_output(_dummy in 0u32..1) {
                let edges = trace_data_flow(&[]);
                prop_assert!(edges.is_empty());
            }
        }
    }

    // =======================================================================
    // IR-based flow parity tests
    // =======================================================================

    mod ir_parity {
        use super::*;
        use crate::ir::IrFile;

        /// Helper: run heuristic analysis via both paths.
        fn analyze_both(files: &[(&str, &str)]) -> (FlowAnalysis, FlowAnalysis) {
            let parsed: Vec<ParsedFile> = files
                .iter()
                .map(|(path, source)| ast::parse_file(path, source).unwrap())
                .collect();
            let ir_files: Vec<IrFile> = parsed.iter().map(IrFile::from_parsed_file).collect();
            let config = FlowConfig::default();

            let from_parsed = analyze_data_flow(&parsed, &config);
            let from_ir = analyze_data_flow_ir(&ir_files, &config);
            (from_parsed, from_ir)
        }

        #[test]
        fn test_ir_parity_heuristic_db_write() {
            let (fp, fi) = analyze_both(&[(
                "src/repo.ts",
                r#"
import { PrismaClient } from '@prisma/client';
const prisma = new PrismaClient();
async function createUser(data: any) {
    await prisma.user.create({ data });
}
"#,
            )]);

            assert_eq!(
                fp.heuristic_edges.len(),
                fi.heuristic_edges.len(),
                "heuristic edge count should match for DB write"
            );
            for (a, b) in fp.heuristic_edges.iter().zip(fi.heuristic_edges.iter()) {
                assert_eq!(a.pattern, b.pattern);
                assert_eq!(a.file, b.file);
            }
        }

        #[test]
        fn test_ir_parity_heuristic_event_emission() {
            let (fp, fi) = analyze_both(&[(
                "src/events.ts",
                r#"
import { EventEmitter } from 'events';
const emitter = new EventEmitter();
function notifyUser() {
    emitter.emit('user:created');
}
"#,
            )]);

            assert_eq!(fp.heuristic_edges.len(), fi.heuristic_edges.len());
        }

        #[test]
        fn test_ir_parity_framework_detection() {
            let files = &[
                (
                    "src/app.ts",
                    r#"
import express from 'express';
import { PrismaClient } from '@prisma/client';
"#,
                ),
                (
                    "src/views.py",
                    r#"
from flask import Flask
from sqlalchemy import Column
"#,
                ),
            ];

            let parsed: Vec<ParsedFile> = files
                .iter()
                .map(|(path, source)| ast::parse_file(path, source).unwrap())
                .collect();
            let ir_files: Vec<IrFile> = parsed.iter().map(IrFile::from_parsed_file).collect();

            let from_parsed = detect_frameworks(&parsed);
            let from_ir = detect_frameworks_ir(&ir_files);

            assert_eq!(
                from_parsed, from_ir,
                "framework detection should match: {:?} vs {:?}",
                from_parsed, from_ir
            );
        }

        #[test]
        fn test_ir_parity_empty() {
            let (fp, fi) = analyze_both(&[]);
            assert_eq!(fp.heuristic_edges.len(), fi.heuristic_edges.len());
            assert_eq!(fp.frameworks_detected, fi.frameworks_detected);
        }

        #[test]
        fn test_ir_parity_logging_detection() {
            let (fp, fi) = analyze_both(&[(
                "src/utils.ts",
                r#"
function doStuff() {
    console.log('doing stuff');
    console.error('oops');
}
"#,
            )]);

            assert_eq!(fp.heuristic_edges.len(), fi.heuristic_edges.len());
            for (a, b) in fp.heuristic_edges.iter().zip(fi.heuristic_edges.iter()) {
                assert_eq!(a.pattern, b.pattern);
            }
        }
    }

    // =======================================================================
    // IR-based data flow edge tests
    // =======================================================================

    mod ir_data_flow {
        use super::*;
        use crate::ast;
        use crate::ir::IrFile;

        /// Helper: parse a file to IR and build data flow edges.
        fn ir_edges(path: &str, source: &str) -> Vec<DataFlowEdge> {
            let parsed = ast::parse_file(path, source).unwrap();
            let data_flow = ast::extract_data_flow_info(path, source).unwrap();
            let mut ir = IrFile::from_parsed_file(&parsed);
            ir.enrich_with_data_flow(&data_flow);
            build_data_flow_edges_from_ir(&ir)
        }

        #[test]
        fn test_ir_data_flow_simple_chain() {
            let edges = ir_edges(
                "src/handler.ts",
                r#"
function handler() {
    const data = fetchData();
    process(data);
}
"#,
            );

            assert_eq!(edges.len(), 1, "should find one data flow edge");
            assert_eq!(edges[0].producer, "fetchData");
            assert_eq!(edges[0].consumer, "process");
            assert_eq!(edges[0].via, "data");
        }

        #[test]
        fn test_ir_data_flow_multiple_consumers() {
            let edges = ir_edges(
                "src/handler.ts",
                r#"
function handler() {
    const data = fetchData();
    validate(data);
    transform(data);
}
"#,
            );

            assert_eq!(edges.len(), 2, "should find two data flow edges");
            assert!(edges.iter().any(|e| e.consumer == "validate"));
            assert!(edges.iter().any(|e| e.consumer == "transform"));
        }

        #[test]
        fn test_ir_data_flow_pipeline() {
            let edges = ir_edges(
                "src/handler.ts",
                r#"
function handler() {
    const raw = fetch();
    const clean = sanitize(raw);
    save(clean);
}
"#,
            );

            assert!(edges.len() >= 2, "should find pipeline edges");
            assert!(edges
                .iter()
                .any(|e| e.producer == "fetch" && e.consumer == "sanitize" && e.via == "raw"));
            assert!(edges
                .iter()
                .any(|e| e.producer == "sanitize" && e.consumer == "save" && e.via == "clean"));
        }

        #[test]
        fn test_ir_data_flow_scope_isolation() {
            let edges = ir_edges(
                "src/app.ts",
                r#"
function funcA() {
    const x = fetch();
    process(x);
}
function funcB() {
    const x = other();
    transform(x);
}
"#,
            );

            // Should not cross function boundaries
            for edge in &edges {
                if edge.producer == "fetch" {
                    assert_eq!(edge.consumer, "process");
                }
                if edge.producer == "other" {
                    assert_eq!(edge.consumer, "transform");
                }
            }
        }

        #[test]
        fn test_ir_data_flow_no_self_edge() {
            let edges = ir_edges(
                "src/app.ts",
                r#"
function handler() {
    const data = fetch();
    fetch(data);
}
"#,
            );

            for edge in &edges {
                assert_ne!(
                    edge.producer, edge.consumer,
                    "should not produce self-edges"
                );
            }
        }

        #[test]
        fn test_ir_data_flow_empty() {
            let edges = ir_edges(
                "src/empty.ts",
                r#"
function handler() {
    console.log('hello');
}
"#,
            );

            assert!(edges.is_empty(), "no data flow edges in simple code");
        }

        #[test]
        fn test_ir_data_flow_parity_with_parsed() {
            let source = r#"
function handler() {
    const data = fetchData();
    process(data);
}
"#;
            let path = "src/handler.ts";

            // ParsedFile path
            let data_flow_info = ast::extract_data_flow_info(path, source).unwrap();
            let edges_parsed = build_data_flow_edges(&data_flow_info, path);

            // IR path
            let edges_ir = ir_edges(path, source);

            assert_eq!(
                edges_parsed.len(),
                edges_ir.len(),
                "edge count should match: parsed={}, ir={}",
                edges_parsed.len(),
                edges_ir.len()
            );

            for (a, b) in edges_parsed.iter().zip(edges_ir.iter()) {
                assert_eq!(a.producer, b.producer);
                assert_eq!(a.consumer, b.consumer);
                assert_eq!(a.via, b.via);
                assert_eq!(a.file, b.file);
            }
        }

        #[test]
        fn test_trace_data_flow_ir_empty() {
            let edges = trace_data_flow_ir(&[]);
            assert!(edges.is_empty());
        }

        #[test]
        fn test_trace_data_flow_ir_multiple_files() {
            let files: Vec<(&str, &str)> = vec![
                (
                    "src/a.ts",
                    r#"
function processA() {
    const x = fetchA();
    saveA(x);
}
"#,
                ),
                (
                    "src/b.ts",
                    r#"
function processB() {
    const y = fetchB();
    saveB(y);
}
"#,
                ),
            ];

            let ir_files: Vec<IrFile> = files
                .iter()
                .map(|(path, source)| {
                    let parsed = ast::parse_file(path, source).unwrap();
                    let data_flow = ast::extract_data_flow_info(path, source).unwrap();
                    let mut ir = IrFile::from_parsed_file(&parsed);
                    ir.enrich_with_data_flow(&data_flow);
                    ir
                })
                .collect();

            let edges = trace_data_flow_ir(&ir_files);
            assert!(
                edges.len() >= 2,
                "should find edges from both files, got {}",
                edges.len()
            );
            assert!(edges.iter().any(|e| e.file == "src/a.ts"));
            assert!(edges.iter().any(|e| e.file == "src/b.ts"));
        }
    }

    // =======================================================================
    // IR-based flow property-based tests
    // =======================================================================

    mod ir_proptests {
        use super::*;
        use crate::ast;
        use crate::ir::IrFile;
        use proptest::prelude::*;

        fn func_name() -> impl Strategy<Value = String> {
            "[a-z][a-zA-Z]{0,8}".prop_map(|s| s)
        }

        proptest! {
            #[test]
            fn prop_ir_data_flow_never_panics(
                func in func_name(),
                var in func_name(),
                callee1 in func_name(),
                callee2 in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee1, callee2, var
                );
                let parsed = ast::parse_file("test.ts", &source).unwrap();
                let data_flow = ast::extract_data_flow_info("test.ts", &source).unwrap();
                let mut ir = IrFile::from_parsed_file(&parsed);
                ir.enrich_with_data_flow(&data_flow);
                let _ = build_data_flow_edges_from_ir(&ir);
            }

            #[test]
            fn prop_ir_data_flow_no_self_edges(
                func in func_name(),
                var in func_name(),
                callee in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee, callee, var
                );
                let parsed = ast::parse_file("test.ts", &source).unwrap();
                let data_flow = ast::extract_data_flow_info("test.ts", &source).unwrap();
                let mut ir = IrFile::from_parsed_file(&parsed);
                ir.enrich_with_data_flow(&data_flow);
                let edges = build_data_flow_edges_from_ir(&ir);
                for edge in &edges {
                    prop_assert!(edge.producer != edge.consumer,
                        "self-edge: {} → {}", edge.producer, edge.consumer);
                }
            }

            #[test]
            fn prop_ir_data_flow_valid_fields(
                func in func_name(),
                var in func_name(),
                callee1 in func_name(),
                callee2 in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee1, callee2, var
                );
                let parsed = ast::parse_file("test.ts", &source).unwrap();
                let data_flow = ast::extract_data_flow_info("test.ts", &source).unwrap();
                let mut ir = IrFile::from_parsed_file(&parsed);
                ir.enrich_with_data_flow(&data_flow);
                let edges = build_data_flow_edges_from_ir(&ir);
                for edge in &edges {
                    prop_assert!(!edge.producer.is_empty());
                    prop_assert!(!edge.consumer.is_empty());
                    prop_assert!(!edge.via.is_empty());
                    prop_assert!(!edge.file.is_empty());
                    prop_assert!(edge.line > 0);
                }
            }

            #[test]
            fn prop_ir_data_flow_deterministic(
                func in func_name(),
                var in func_name(),
                callee1 in func_name(),
                callee2 in func_name()
            ) {
                let source = format!(
                    "function {}() {{\n  const {} = {}();\n  {}({});\n}}\n",
                    func, var, callee1, callee2, var
                );
                let parsed = ast::parse_file("test.ts", &source).unwrap();
                let data_flow = ast::extract_data_flow_info("test.ts", &source).unwrap();
                let mut ir1 = IrFile::from_parsed_file(&parsed);
                ir1.enrich_with_data_flow(&data_flow);
                let mut ir2 = IrFile::from_parsed_file(&parsed);
                ir2.enrich_with_data_flow(&data_flow);
                let e1 = build_data_flow_edges_from_ir(&ir1);
                let e2 = build_data_flow_edges_from_ir(&ir2);
                prop_assert_eq!(e1, e2);
            }

            #[test]
            fn prop_ir_heuristic_parity(
                func in func_name(),
                callee1 in func_name()
            ) {
                // Build a simple file and check that heuristic analysis matches
                let source = format!(
                    "function {}() {{\n  {}();\n}}\n",
                    func, callee1
                );
                let parsed: Vec<ParsedFile> = vec![ast::parse_file("test.ts", &source).unwrap()];
                let ir_files: Vec<IrFile> = parsed.iter().map(IrFile::from_parsed_file).collect();
                let config = FlowConfig::default();

                let fp = analyze_data_flow(&parsed, &config);
                let fi = analyze_data_flow_ir(&ir_files, &config);

                prop_assert_eq!(fp.heuristic_edges.len(), fi.heuristic_edges.len());
                prop_assert_eq!(fp.frameworks_detected, fi.frameworks_detected);
            }

            #[test]
            fn prop_ir_trace_empty(_dummy in 0u32..1) {
                let edges = trace_data_flow_ir(&[]);
                prop_assert!(edges.is_empty());
            }
        }
    }
}
