//! Local HTTP server for engine tests. `cargo run -p testserver -- --port 8787`

use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut port = 8787;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--port", Some(p)) => port = p.parse().expect("--port takes a number"),
            _ => {
                eprintln!("usage: testserver [--port N]");
                std::process::exit(2);
            }
        }
    }
    // Loopback only: this server is for local tests, never for the network.
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    println!("testserver on http://{}/", listener.local_addr()?);
    testserver::serve(listener).await
}
