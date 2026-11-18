// SPDX-License-Identifier: GPL-3.0-or-later
//! Stdio adapter for the compiler's editor syntax products.

mod presentation;

use std::io::{self, BufRead, BufWriter, Write};

use serde::{Deserialize, Serialize};

use presentation::Presentation;

#[derive(Deserialize)]
struct Request {
    id: u64,
    text: String,
}

#[derive(Serialize)]
struct Response {
    id: u64,
    result: Option<Presentation>,
    error: Option<String>,
}

fn main() -> io::Result<()> {
    let mut output = BufWriter::new(io::stdout().lock());
    let mut input = io::stdin().lock();
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let request: Request = serde_json::from_str(&line)?;
        let result = nia_syntax::parse(&request.text, None)
            .map(|parsed| Presentation::new(&parsed.tree))
            .map_err(|error| format!("{error:?}"));
        let response = match result {
            Ok(result) => Response {
                id: request.id,
                result: Some(result),
                error: None,
            },
            Err(error) => Response {
                id: request.id,
                result: None,
                error: Some(error),
            },
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}
