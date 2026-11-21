//! Embedded API documentation: OpenAPI 3.0 JSON + a lightweight index page.

/// OpenAPI 3.0.3 description served at `/api-docs/openapi.json` (I-1).
pub const OPENAPI_JSON: &str = include_str!("../docs/openapi.json");

/// Minimal, dependency-free HTML index listing the API and linking the
/// spec; renders the endpoint table from the embedded JSON at load time.
pub const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en" dir="ltr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>PortOS API</title>
<style>
  :root { color-scheme: light dark; }
  body { font-family: system-ui, "Vazirmatn", sans-serif; margin: 0 auto; max-width: 60rem; padding: 1.5rem; }
  h1 { font-size: 1.6rem; } code { background: #f0f1f3; padding: .1rem .35rem; border-radius: 4px; }
  table { border-collapse: collapse; width: 100%; font-size: .92rem; }
  th, td { text-align: left; padding: .35rem .5rem; border-bottom: 1px solid #8884; white-space: nowrap; }
  td.method { font-weight: 700; color: #0a7d32; }
  @media (max-width: 640px) { table { font-size: .8rem; } }
</style>
</head>
<body>
<h1>PortOS API — v1</h1>
<p>OpenAPI 3.0 spec: <a href="/api-docs/openapi.json">/api-docs/openapi.json</a>.
Health: <code>/healthz</code>, <code>/readyz</code>, <code>/metrics</code>.
All protected endpoints require <code>Authorization: Bearer &lt;token&gt;</code>
from <code>POST /v1/auth/login</code>. Locale: <code>?lang=fa</code>,
calendar: <code>?calendar=jalali</code>, numerals: <code>?numerals=persian</code>.</p>
<table id="paths"><thead><tr><th>Method</th><th>Path</th><th>Summary</th></tr></thead><tbody></tbody></table>
<script>
fetch("/api-docs/openapi.json").then(r => r.json()).then(spec => {
  const tbody = document.querySelector("#paths tbody");
  for (const [path, methods] of Object.entries(spec.paths || {})) {
    for (const [method, op] of Object.entries(methods)) {
      const tr = document.createElement("tr");
      tr.innerHTML = `<td class="method">${method.toUpperCase()}</td><td><code>${path}</code></td><td>${op.summary || ""}</td>`;
      tbody.appendChild(tr);
    }
  }
}).catch(() => {});
</script>
</body>
</html>
"##;
