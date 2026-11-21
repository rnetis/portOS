//! PortOS server binary: `portosd serve | init-db | seed-demo | hash-password | version`.
//!
//! Production startup path: `serve` opens the SQLite database, applies
//! migrations, loads i18n catalogs (with override directories — L-10),
//! binds the listener and serves until SIGINT/SIGTERM with graceful
//! shutdown (§6.5 zero-downtime upgrades pair with the readiness probe).

use portos_api::{auth, config::{Config, LogFormat}, events, routes, seed, state::AppState};
use portos_core::{CoreError, CoreResult};

fn init_tracing(cfg: &Config) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,portosd=debug"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_line_number(false);
    match cfg.log_format {
        LogFormat::Json => {
            builder.json().flatten_event(true).init();
        }
        LogFormat::Pretty => {
            builder.pretty().init();
        }
    }
}

fn open_configured_store(cfg: &Config) -> CoreResult<portos_store::Store> {
    portos_store::Store::open(&cfg.db_path)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "serve".to_string());
    let rest: Vec<String> = args.collect();

    let result = match cmd.as_str() {
        "serve" => serve(),
        "init-db" => init_db(&rest),
        "seed-demo" => seed_demo(&rest),
        "hash-password" => hash_password_cmd(rest.first().map(String::as_str)),
        "version" => {
            println!("portosd {} (event schema {})", portos_core::VERSION, portos_core::EVENT_SCHEMA_VERSION);
            Ok(())
        }
        "--help" | "help" | "-h" => {
            print_help();
            Ok(())
        }
        other => {
            eprintln!("unknown command {other:?}");
            print_help();
            Err(CoreError::Validation("unknown command".into()))
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "portosd — PortOS server\n\n\
         USAGE:\n  \
         portosd serve                       start the HTTP server (PORTOS_* env config)\n  \
         portosd init-db [--db PATH]         create the database and apply migrations\n  \
         portosd seed-demo [--db PATH]       load realistic demo data (users, yard, calls, KPIs)\n  \
         portosd hash-password <pw>          print an Argon2id hash for scripting\n  \
         portosd version                     print version\n\n\
         KEY ENV: PORTOS_BIND_ADDR, PORTOS_DB_PATH, PORTOS_JWT_SECRET, PORTOS_TENANT,\n         \
         PORTOS_LOG_FORMAT=json|pretty, PORTOS_I18N_EXTRA_DIRS, PORTOS_REQUIRE_SECRET=1"
    );
}

fn serve() -> CoreResult<()> {
    let cfg = Config::from_env()?;
    init_tracing(&cfg);
    let store = open_configured_store(&cfg)?;
    let state = AppState::new(store, cfg.clone());
    // Load embedded catalogs + override directories (L-10).
    match state.reload_catalogs() {
        Ok(warnings) => {
            for w in warnings {
                tracing::warn!(warning = %w, "i18n override");
            }
        }
        Err(e) => return Err(CoreError::Internal(format!("i18n catalogs: {e}"))),
    }
    state.set_ready(true);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CoreError::Internal(format!("tokio: {e}")))?;
    runtime.block_on(run_server(state))
}

async fn run_server(state: AppState) -> CoreResult<()> {
    let addr = state.config.bind_addr.clone();
    let app = routes::build(state.clone());

    // Webhook dispatcher runs beside the server for the process lifetime.
    let webhook_state = state.clone();
    tokio::spawn(events::run_webhook_worker(webhook_state));
    // Rate-limiter janitor: drop stale buckets every minute.
    let janitor_state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tick.tick().await;
            janitor_state.global_limiter.sweep(std::time::Duration::from_secs(600));
            janitor_state.login_limiter.sweep(std::time::Duration::from_secs(3600));
        }
    });

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| CoreError::Internal(format!("bind {addr}: {e}")))?;
    tracing::info!(
        bind = %addr,
        tenant = %state.config.tenant,
        version = portos_core::VERSION,
        "PortOS server listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| CoreError::Internal(format!("serve: {e}")))?;
    tracing::info!("shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

fn init_db(rest: &[String]) -> CoreResult<()> {
    let db_path = flag_value(rest, "--db").map(std::path::PathBuf::from).unwrap_or_else(|| {
        Config::from_env().map(|c| c.db_path).unwrap_or_else(|_| std::path::PathBuf::from("data/portos.db"))
    });
    let store = portos_store::Store::open(&db_path)?;
    println!("database ready at {} (migrations applied)", db_path.display());
    let _ = store;
    Ok(())
}

fn seed_demo(rest: &[String]) -> CoreResult<()> {
    let cfg = Config::from_env()?;
    init_tracing(&cfg);
    let db_path = flag_value(rest, "--db").map(std::path::PathBuf::from).unwrap_or_else(|| cfg.db_path.clone());
    let store = portos_store::Store::open(&db_path)?;
    let summary = seed::seed_demo(&store, &cfg.tenant)?;
    println!("demo data loaded into {}:\n{summary}", db_path.display());
    println!(
        "\nDemo logins (argon2-hashed in DB; change in production):\n  admin / Admin!2026\n  planner / Planner!2026\n  gateclerk / Gate!2026\n  customs / Customs!2026\n  finance / Finance!2026"
    );
    Ok(())
}

fn hash_password_cmd(pw: Option<&str>) -> CoreResult<()> {
    let Some(pw) = pw else {
        return Err(CoreError::Validation("usage: portosd hash-password <password>".into()));
    };
    let hash = auth::hash_password(pw)?;
    println!("{hash}");
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
