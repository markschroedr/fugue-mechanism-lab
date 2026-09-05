use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, port) = fugue_lab::bind_address_from_env()?;
    let address = SocketAddr::new(host, port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("fugue_lab_listening={address}");
    axum::serve(listener, fugue_lab::app_from_env()?).await?;
    Ok(())
}
