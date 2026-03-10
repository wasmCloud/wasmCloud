use hello_world::HelloRequest;
use hello_world::greeter_client::GreeterClient;

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bin-client".to_string());
    let mut client = GreeterClient::connect("http://localhost:8000").await?;
    let request = tonic::Request::new(HelloRequest { name });
    let response = client.say_hello(request).await?;
    println!("{}", response.into_inner().message);
    Ok(())
}
