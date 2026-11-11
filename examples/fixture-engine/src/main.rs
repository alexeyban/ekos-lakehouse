//! `ekos-lakehouse-fixture`: the EKOS CLI and the Lakehouse commands, with the fixture engine
//! registered — the shape of a private binary with a sold engine (RFC 0255 §3.3).

use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ekos::app::main_with(ekos::extension::Extensions::new(vec![Arc::new(
        ekos_lakehouse_fixture_engine::lakehouse(),
    )]))
    .await
}
