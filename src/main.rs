use futures::future;
use futures::stream::StreamExt;
use futures::FutureExt;
use http::header::{HeaderMap, HeaderValue};
use http::status::StatusCode;
use http::Uri;
use hyper::service::{make_service_fn, service_fn};
use hyper::{header, Body, Method, Request, Response, Server};
use std::error::Error as StdError;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::{env, io};
use structopt::StructOpt;
use thiserror::Error;
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, warn};

fn main() {
    // Set up error handling immediately
    if let Err(e) = run() {
        log_error_chain(&e);
    }
}

/// Basic error reporting, including the "cause chain". This is used both by the
/// top-level error reporting and to report internal server errors.
fn log_error_chain(mut e: &dyn StdError) {
    error!("error: {}", e);
    while let Some(source) = e.source() {
        error!("caused by: {}", source);
        e = source;
    }
}

#[derive(Clone, StructOpt)]
#[structopt(about = "A basic HTTP file server")]
pub struct Config {
    /// The IP:PORT combination.
    #[structopt(
        name = "ADDR",
        short = "a",
        long = "addr",
        parse(try_from_str),
        default_value = "127.0.0.1:4000"
    )]
    addr: SocketAddr,

    /// The root directory for serving files.
    #[structopt(name = "ROOT", parse(from_os_str), default_value = ".")]
    root_dir: PathBuf,
}

fn run() -> Result<()> {
    // Initialize logging, and log the "info" level for this crate only, unless
    // the environment contains `RUST_LOG`.
    let file_appender = tracing_appender::rolling::hourly("/var/log", "sufficient.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    let mut base_config = tracing_subscriber::fmt();

    if env::var("NO_ANSI").is_ok() {
        base_config = base_config.with_ansi(false);
    }
    let base_config = base_config.with_writer(non_blocking);

    base_config.init();

    // Create the configuration from the command line arguments. It
    // includes the IP address and port to listen on and the path to use
    // as the HTTP server's root directory.
    let config = Config::from_args();

    // Display the configuration to be helpful
    info!("sufficient {}", env!("CARGO_PKG_VERSION"));
    info!("addr: http://{}", config.addr);
    info!("root dir: {}", config.root_dir.display());

    // Create the MakeService object that creates a new Hyper service for every
    // connection. Both these closures need to return a Future of Result, and we
    // use two different mechanisms to achieve that.
    let make_service = make_service_fn(|_| {
        let config = config.clone();

        let service = service_fn(move |req| {
            let config = config.clone();

            // Handle the request, returning a Future of Response,
            // and map it to a Future of Result of Response.
            serve(config, req).map(Ok::<_, Error>)
        });

        // Convert the concrete (non-future) service function to a Future of Result.
        future::ok::<_, Error>(service)
    });

    // Create a Hyper Server, binding to an address, and use
    // our service builder.
    let server = Server::bind(&config.addr).serve(make_service);

    // Create a Tokio runtime and block on Hyper forever.
    let rt = Runtime::new()?;
    rt.block_on(server)?;

    Ok(())
}

/// Create an HTTP Response future for each Request.
///
/// Errors are turned into an appropriate HTTP error response, and never
/// propagated upward for hyper to deal with.
async fn serve(config: Config, req: Request<Body>) -> Response<Body> {
    // Serve the requested file.
    let resp = serve_or_error(config, req).await;

    // Transform internal errors to error responses.
    let resp = transform_error(resp);

    resp
}

/// A custom `Result` typedef
pub type Result<T> = std::result::Result<T, Error>;

/// The basic-http-server error type.
///
/// This is divided into two types of errors: "semantic" errors and "blanket"
/// errors. Semantic errors are custom to the local application semantics and
/// are usually preferred, since they add context and meaning to the error
/// chain. They don't require boilerplate `From` implementations, but do require
/// `map_err` to create when they have interior `causes`.
///
/// Blanket errors are just wrappers around other types, like `Io(io::Error)`.
/// These are common errors that occur in many places so are easier to code and
/// maintain, since e.g. every occurrence of an I/O error doesn't need to be
/// given local semantics.
///
/// The criteria of when to use which type of error variant, and their pros and
/// cons, aren't obvious.
///
/// These errors use `derive(Display)` from the `derive-more` crate to reduce
/// boilerplate.
#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP error")]
    Http(http::Error),

    #[error("Hyper error")]
    Hyper(hyper::Error),

    #[error("I/O error")]
    Io(io::Error),

    // custom "semantic" error types
    #[error("failed to parse IP address")]
    AddrParse(std::net::AddrParseError),

    #[error("requested URI is not an absolute path")]
    UriNotAbsolute,

    #[error("requested URI is not UTF-8")]
    UriNotUtf8,
}
