use tokio_postgres::NoTls;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("ERROR: {}", e);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<String>>();
    if args.len() < 2 {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "missing uri parameter").into());
    }
    let uri = args.get(1).unwrap();
    println!("connecting to: {}", uri);
    let (client, conn) = tokio_postgres::connect(uri, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("connection error: {}", e);
        }
    });

    let query = if args.len() >= 3 {
        args[2..].join(" ")
    } else {
        "select 1".to_string()
    };
    match query.split_once(' ').unwrap().0.to_lowercase().as_str() {
        "select" => {
            println!("executing query: '{}'", &query);
            let rows = client.query(query.as_str(), &[]).await?;
            println!("{} rows:", rows.len());
            for row in rows.iter() {
                println!("{:?}", row)
            }
        }
        _ => {
            println!("executing ex: '{}'", &query);
            let result = client.execute(query.as_str(), &Vec::new()).await?;
            println!("{} rows affected", result);
        }
    }
    Ok(())
}
