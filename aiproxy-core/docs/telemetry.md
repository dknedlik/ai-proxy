--- aiproxy-core/src/http_client.rs
@@
-pub async fn post_sse_lines<T: Serialize + ?Sized>(
-    &self,
-    url: &str,
-    body: &T,
-    headers: &[(&str, &str)],
-    ctx: &RequestCtx<'_>,
-) -> CoreResult<SseStream> {
+pub async fn post_sse_lines<T: Serialize + ?Sized>(
+    &self,
+    url: &str,
+    body: &T,
+    headers: &[(&str, &str)],
+    ctx: &RequestCtx<'_>,
+) -> CoreResult<(SseStream, Option<String>)> {
@@
-    Ok(Box::pin(wrapped))
+    Ok((Box::pin(wrapped), provider_request_id.clone()))
@@
-#[tokio::test]
-async fn post_sse_lines_emits_telemetry_on_completion() {
-    let client = test_client();
-    let mut stream = client
-        .post_sse_lines("http://example.com", &(), &[], &RequestCtx::default())
-        .await
-        .expect("sse ok");
+#[tokio::test]
+async fn post_sse_lines_emits_telemetry_on_completion() {
+    let client = test_client();
+    let (mut stream, _pid) = client
+        .post_sse_lines("http://example.com", &(), &[], &RequestCtx::default())
+        .await
+        .expect("sse ok");
@@
-#[tokio::test]
-async fn sse_early_drop_records_latency() {
-    let client = test_client();
-    let mut stream = client
-        .post_sse_lines("http://example.com", &(), &[], &RequestCtx::default())
-        .await
-        .expect("sse ok");
+#[tokio::test]
+async fn sse_early_drop_records_latency() {
+    let client = test_client();
+    let (mut stream, _pid) = client
+        .post_sse_lines("http://example.com", &(), &[], &RequestCtx::default())
+        .await
+        .expect("sse ok");
@@
-#[tokio::test]
-async fn sse_server_closes_without_done_records_latency_once() {
-    let client = test_client();
-    let mut stream = client
-        .post_sse_lines("http://example.com", &(), &[], &RequestCtx::default())
-        .await
-        .expect("sse ok");
+#[tokio::test]
+async fn sse_server_closes_without_done_records_latency_once() {
+    let client = test_client();
+    let (mut stream, _pid) = client
+        .post_sse_lines("http://example.com", &(), &[], &RequestCtx::default())
+        .await
+        .expect("sse ok");
@@
-#[tokio::test]
-async fn sse_headers_include_accept_and_ctx_ids() {
-    let client = test_client();
-    let mut stream = client
-        .post_sse_lines("http://example.com", &(), &[], &RequestCtx::default())
-        .await
-        .expect("sse ok");
+#[tokio::test]
+async fn sse_headers_include_accept_and_ctx_ids() {
+    let client = test_client();
+    let (mut stream, _pid) = client
+        .post_sse_lines("http://example.com", &(), &[], &RequestCtx::default())
+        .await
+        .expect("sse ok");
--- aiproxy-core/src/providers/openai/mod.rs
@@
-        let mut sse = self
-            .http
-            .post_sse_lines(&url, &payload, &hdrs, &ctx)
-            .await?;
+        let (mut sse, provider_request_id) = self
+            .http
+            .post_sse_lines(&url, &payload, &hdrs, &ctx)
+            .await?;
@@
-        telemetry::emit_completion(log);
+        telemetry::emit_completion(
+            log.provider_request_id_opt(provider_request_id.as_deref())
+        );
