//! `ekos-lakehouse`: the EKOS CLI, unchanged, plus the `ekos-lakehouse lakehouse …` commands
//! (RFC 0190). Named apart from `ekos` so a machine never has two different `ekos` binaries.

use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ekos::app::main_with(ekos::extension::Extensions::new(vec![Arc::new(
        ekos_lakehouse_ext::LakehouseExtension,
    )]))
    .await
}
