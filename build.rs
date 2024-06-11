fn main() {
    ks_placeholder::placeholder! {"src/private.rs";
        pub struct CodeStaff {}

        impl CodeStaff {
            pub fn start(
                _: teloxide::adaptors::DefaultParseMode<teloxide::Bot>,
                _: crate::database::DatabaseHelper,
                _: tokio::sync::broadcast::Receiver<crate::database::BroadcastEvent>,
            )-> Self {Self {}}

            pub async fn wait(&self) -> anyhow::Result<()> {
                Ok(())
            }
        }
    }
}
