use anyhow::Result;

// Integration tests still need a real executable target to spawn for stdio
// handshakes, but the fixture server itself lives under `tests/support`.
#[path = "../../tests/support/stdio_fixture.rs"]
mod stdio_fixture;

#[tokio::main]
async fn main() -> Result<()> {
    stdio_fixture::run_stdio_fixture_server().await
}
