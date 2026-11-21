//! End-to-end API integration tests. The router is exercised directly via
//! `tower::Service::oneshot` — no sockets, deterministic and fast.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portos_api::config::Config;
use portos_api::state::AppState;
use portos_store::Store;
use tower::Service;
use tower::ServiceExt;

fn test_state() -> (AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("portos-it-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("test.db");
    let cfg = Config {
        bind_addr: "127.0.0.1:0".into(),
        db_path: db_path.clone(),
        jwt_secret: "integration-test-secret-0123456789abcdef".into(),
        jwt_ttl_hours: 1,
        tenant: "TEST-CT1".into(),
        cors_origins: vec!["*".into()],
        rate_limit_rps: 1000.0,
        login_rate_per_min: 100,
        body_limit_bytes: 1_000_000,
        timeout_secs: 10,
        i18n_extra_dirs: vec![],
        default_locale: portos_i18n::Locale::En,
        log_format: portos_api::config::LogFormat::Json,
        webhook_timeout_secs: 5,
        require_secret: false,
    };
    let store = Store::open(&db_path).unwrap();
    portos_api::seed::seed_demo(&store, "TEST-CT1").unwrap();
    (AppState::new(store, cfg), dir)
}

async fn req(
    app: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<String>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(body.unwrap_or_default()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 2_000_000).await.unwrap();
    let json: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn full_operational_flow() {
    let (state, dir) = test_state();
    let mut app = portos_api::routes::build(state.clone());
    state.set_ready(true);

    // --- health & readiness ------------------------------------------------
    let (status, body) = req(&app, "GET", "/healthz", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = req(&app, "GET", "/readyz", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // --- auth: wrong password rejected, right password issues a JWT --------
    let (status, _) = req(
        &app,
        "POST",
        "/v1/auth/login",
        None,
        Some(r#"{"username":"planner","password":"wrong"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = req(
        &app,
        "POST",
        "/v1/auth/login",
        None,
        Some(r#"{"username":"planner","password":"Planner!2026"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["access_token"].as_str().expect("token").to_string();
    assert!(!token.is_empty());

    // Persian users get a localized welcome message.
    assert!(body["message"].as_str().unwrap().chars().any(|c| !c.is_ascii()), "fa message");

    // --- protected route without token → 401 problem+json ------------------
    let (status, body) = req(&app, "GET", "/v1/vessels", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["type"], "https://portos.dev/problems/unauthorized");

    // --- RBAC: gate clerk cannot create vessels ----------------------------
    let (_, clerk_login) = req(
        &app,
        "POST",
        "/v1/auth/login",
        None,
        Some(r#"{"username":"gateclerk","password":"Gate!2026"}"#.into()),
    )
    .await;
    let clerk_token = clerk_login["access_token"].as_str().unwrap().to_string();
    let (status, body) = req(
        &app,
        "POST",
        "/v1/vessels",
        Some(&clerk_token),
        Some(r#"{"imo":"9811000","name_en":"X","name_fa":"ایکس","loa_m":100,"beam_m":20,"max_draft_m":8,"dwt_t":10000}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // --- vessel + call + berth suggestion & allocation ----------------------
    let imo = portos_core::vessel::ImoNumber::from_prefix("957321").unwrap();
    let (status, body) = req(
        &app,
        "POST",
        "/v1/vessels",
        Some(&token),
        Some(serde_json::json!({
            "imo": imo.as_str(),
            "name_en": "MV Test Runner",
            "name_fa": "رانر آزمون",
            "loa_m": 250.0,
            "beam_m": 32.0,
            "max_draft_m": 12.5,
            "dwt_t": 50_000,
            "reefer_points": 300,
            "gearless": true
        }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let vessel_id = body["id"].as_str().unwrap().to_string();

    let eta = (chrono::Utc::now() + chrono::Duration::hours(6)).to_rfc3339();
    let (status, body) = req(
        &app,
        "POST",
        "/v1/vessel-calls",
        Some(&token),
        Some(serde_json::json!({
            "vessel_id": vessel_id,
            "voyage_code": "IT001E",
            "port": "IRBND",
            "eta_utc": eta,
            "cargo_mix": {"discharge_teu": 900, "load_teu": 200}
        }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let call_id = body["id"].as_str().unwrap().to_string();

    // Anchorage arrival + berthing stamps KPI data.
    let (status, body) = req(
        &app,
        "POST",
        &format!("/v1/vessel-calls/{call_id}/status"),
        Some(&token),
        Some(r#"{"status":"anchorage_arrived"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Illegal transition must be refused.
    let (status, _) = req(
        &app,
        "POST",
        &format!("/v1/vessel-calls/{call_id}/status"),
        Some(&token),
        Some(r#"{"status":"departed"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, body) = req(
        &app,
        "POST",
        &format!("/v1/vessel-calls/{call_id}/berth-suggest"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let feasible = body["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| !s["score"].is_null())
        .expect("at least one feasible berth");
    let berth_code = body["suggestions"][feasible]["berth_code"].as_str().unwrap().to_string();

    let (status, body) = req(
        &app,
        "POST",
        &format!("/v1/vessel-calls/{call_id}/berth-allocate"),
        Some(&token),
        Some(serde_json::json!({ "suggestion_index": feasible }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["allocation"]["berth_code"], serde_json::json!(berth_code));
    assert!(body["allocation"]["reasons"].as_array().unwrap().len() > 0);

    // --- yard: container upsert + placement with reasons --------------------
    let cn = portos_core::container::ContainerNumber::from_prefix("ITPU123456").unwrap();
    let (status, body) = req(
        &app,
        "POST",
        "/v1/yard/containers",
        Some(&token),
        Some(serde_json::json!({
            "number": cn.as_str(),
            "size_type": "22G1",
            "weight_kg": 21000,
            "pod": "AEJEA"
        }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = req(
        &app,
        "POST",
        &format!("/v1/yard/containers/{}/place", cn.as_str()),
        Some(&token),
        Some(r#"{"next_leg":"vessel"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["reasons"].as_array().unwrap().len() > 0, "explainable placement");

    // Hold then place → conflict.
    let (status, _) = req(
        &app,
        "POST",
        &format!("/v1/yard/containers/{}/hold", cn.as_str()),
        Some(&token),
        Some(r#"{"kind":"customs","reason":"scan"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req(
        &app,
        "POST",
        &format!("/v1/yard/containers/{}/place", cn.as_str()),
        Some(&token),
        Some(r#"{"next_leg":"vessel"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // --- gate: appointment + transaction with idempotency ------------------
    let slot_from = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let slot_to = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let (status, body) = req(
        &app,
        "POST",
        "/v1/gate/appointments",
        Some(&token),
        Some(serde_json::json!({
            "driver_name": "Ali Rezaei",
            "driver_license": "DL-IT-1",
            "plate": "22A99999",
            "container": cn.as_str(),
            "direction": "export",
            "slot_from": slot_from,
            "slot_to": slot_to
        }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let appt_ref = body["appointment"]["reference"].as_str().unwrap().to_string();

    let start_body = serde_json::json!({
        "direction": "export",
        "appointment_reference": appt_ref,
        "plate": "22A99999",
        "driver_license": "DL-IT-1",
        "gate_lane": "L1"
    })
    .to_string();
    let (status, body) = req(
        &app,
        "POST",
        "/v1/gate/transactions/start",
        Some(&clerk_token),
        Some(start_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tx_id = body["transaction"]["id"].as_str().unwrap().to_string();

    // Wrong step order rejected.
    let (status, _) = req(
        &app,
        "POST",
        &format!("/v1/gate/transactions/{tx_id}/step"),
        Some(&clerk_token),
        Some(r#"{"kind":"ocr_scan","ok":true}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Walk the export flow: appointment → driver → OCR → seal → release → ramp out.
    for (kind, ocr) in [
        ("appointment_check", None),
        ("driver_id", None),
        ("ocr_scan", Some(cn.as_str().to_string())),
        ("seal_check", None),
        ("release_check", None),
        ("ramp_out", None),
    ] {
        let payload = serde_json::json!({
            "kind": kind,
            "ok": true,
            "ocr_container": ocr
        })
        .to_string();
        let (status, body) = req(
            &app,
            "POST",
            &format!("/v1/gate/transactions/{tx_id}/step"),
            Some(&clerk_token),
            Some(payload),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    // Completed: gate pass + EIR issued, container marked gate-out.
    assert!(body["issued"]["pass_no"].as_str().is_some(), "{body}");
    assert!(body["elapsed_seconds"].as_f64().unwrap() < 5.0, "gate flow under 5s target");

    // --- billing: tariff, FX, invoice generation, payment ------------------
    let (status, body) = req(
        &app,
        "POST",
        "/v1/billing/invoices/generate",
        Some(&token),
        Some(serde_json::json!({ "container": cn.as_str() }).to_string()),
    )
    .await;
    // Container was gate-out; storage needs a gate-in → seeded data may
    // include this box only via placement, so either OK or a validation
    // error is acceptable; assert it is never a server error.
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    let (status, body) = req(
        &app,
        "POST",
        "/v1/billing/invoices/generate",
        Some(&token),
        Some(serde_json::json!({
            "customer_id": body_customer(&app).await,
            "container": seed_storage_container(&app).await
        }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let invoice_no = body["invoice"]["number"].as_str().unwrap().to_string();
    assert!(body["invoice"]["total_minor"].as_i64().unwrap() > 0);

    let (status, body) = req(
        &app,
        "GET",
        &format!("/v1/billing/invoices/{invoice_no}?lang=fa&calendar=jalali"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Localized display shows Jalali digits for fa clients (L-3/L-4).
    let total_fa = body["display"]["total"].as_str().unwrap();
    assert!(total_fa.chars().any(|c| ('\u{06F0}'..='\u{06F9}').contains(&c)), "{total_fa}");

    let (status, body) = req(
        &app,
        "POST",
        &format!("/v1/billing/invoices/{invoice_no}/payments"),
        Some(&token),
        Some(r#"{"amount_minor": 1000000, "method": "gateway", "reference": "TX-1"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // --- KPI dashboard ------------------------------------------------------
    let (status, body) = req(&app, "GET", "/v1/kpi?hours=72", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["kpis"]["gross_moves_per_hour"].as_f64().unwrap() > 0.0);
    assert!(body["kpis"]["yard_occupancy_pct"].as_f64().unwrap() > 0.0);

    // --- EDI: COPARN import round trip --------------------------------------
    let raw = format!(
        "UNA:+.? '\nUNB+UNOC:3+LINE:ZZZ+IRBNDCT:ZZZ+261121:0800+MSGT1'\n\
         UNH+MSGT1+COPARN:D:96A:UN'\n\
         TDT+20+IT01E+++SEA:172:20+TEST RUNNER:146:20'\n\
         LOC+5+IRBND'\n\
         EQD+CN+{}+22G1+5'\n\
         STS+1'\n\
         UNT+7+MSGT1'\nUNZ+1+MSGT1'",
        portos_core::container::ContainerNumber::from_prefix("EDIP123456").unwrap().as_str()
    );
    let (status, body) = req(
        &app,
        "POST",
        "/v1/edi/import",
        Some(&token),
        Some(serde_json::json!({"message_type":"COPARN","raw":raw}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["containers"], serde_json::json!(1));

    // --- audit trail records the actions (F-40) -----------------------------
    let (_, admin_login) = req(
        &app,
        "POST",
        "/v1/auth/login",
        None,
        Some(r#"{"username":"admin","password":"Admin!2026"}"#.into()),
    )
    .await;
    let admin_token = admin_login["access_token"].as_str().unwrap().to_string();
    let (status, body) = req(
        &app,
        "GET",
        "/v1/admin/audit?limit=50",
        Some(&admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let actions: Vec<&str> = body["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["action"].as_str())
        .collect();
    assert!(actions.contains(&"berth.allocate"), "audit has berth.allocate: {actions:?}");
    assert!(actions.iter().any(|a| a.starts_with("gate.tx")), "audit has gate actions");

    // --- i18n catalog endpoint (L-10) ---------------------------------------
    let (status, body) = req(&app, "GET", "/v1/i18n/catalog?locale=fa", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rtl"], serde_json::json!(true));
    assert!(body["entries"]["gate.pass.title"].as_str().unwrap().contains("قبض"));

    let _ = std::fs::remove_dir_all(dir);
}

/// Find a seeded customer id (finance role cannot create; read via list).
async fn body_customer(app: &axum::Router) -> String {
    let (_, finance) = req(
        app,
        "POST",
        "/v1/auth/login",
        None,
        Some(r#"{"username":"finance","password":"Finance!2026"}"#.into()),
    )
    .await;
    let token = finance["access_token"].as_str().unwrap().to_string();
    let (status, body) = req(app, "GET", "/v1/billing/customers", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    body["customers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "line")
        .and_then(|c| c["id"].as_str())
        .expect("seeded line customer")
        .to_string()
}

/// Find a seeded container that is still in the yard (has a gate-in time).
async fn seed_storage_container(app: &axum::Router) -> String {
    let (_, login) = req(
        app,
        "POST",
        "/v1/auth/login",
        None,
        Some(r#"{"username":"yardplanner","password":"Yard!2026"}"#.into()),
    )
    .await;
    let token = login["access_token"].as_str().unwrap().to_string();
    let (status, body) = req(app, "GET", "/v1/yard/containers?state=in_yard&limit=1", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    body["containers"][0]["number"]
        .as_str()
        .expect("seeded in-yard container")
        .to_string()
}
