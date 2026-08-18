use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

#[derive(serde::Serialize)]
struct Request {
    args: Vec<String>,
}

#[derive(serde::Deserialize)]
struct Response {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

fn main() {
    let socket_path = match env::var("GH_PROXY_SOCKET") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("gh: GH_PROXY_SOCKET not set — gh is not available in this jail");
            std::process::exit(1);
        }
    };

    let args: Vec<String> = env::args().skip(1).collect();

    let mut stream = match UnixStream::connect(&socket_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gh: cannot connect to proxy at {socket_path}: {e}");
            std::process::exit(1);
        }
    };

    let req = Request { args };
    let Ok(mut payload) = serde_json::to_string(&req) else {
        eprintln!("gh: failed to serialize request");
        std::process::exit(1);
    };
    payload.push('\n');
    if let Err(e) = stream.write_all(payload.as_bytes()) {
        eprintln!("gh: failed to send request: {e}");
        std::process::exit(1);
    }

    let reader = BufReader::new(stream);
    let resp_line = match reader.lines().next() {
        Some(Ok(l)) => l,
        Some(Err(e)) => {
            eprintln!("gh: failed to read response: {e}");
            std::process::exit(1);
        }
        None => {
            eprintln!("gh: proxy closed connection without responding");
            std::process::exit(1);
        }
    };

    let resp: Response = match serde_json::from_str(&resp_line) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gh: malformed proxy response: {e}");
            std::process::exit(1);
        }
    };

    print!("{}", resp.stdout);
    eprint!("{}", resp.stderr);
    std::process::exit(resp.exit_code);
}
