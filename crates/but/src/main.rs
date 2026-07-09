#[tokio::main]
async fn main() -> anyhow::Result<()> {
    but_api::fd_limit::raise_soft_limit();
    but_askpass::disable();
    but::handle_args(std::env::args_os()).await
}
