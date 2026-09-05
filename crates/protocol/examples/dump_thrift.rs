use std::{env, fs, process::ExitCode};

use ggfm_protocol::{Field, Value};

fn main() -> ExitCode {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: cargo run -p ggfm-protocol --example dump_thrift -- <packet.bin>");
        return ExitCode::from(2);
    };
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(error) => {
            eprintln!("could not read {}: {error}", path.to_string_lossy());
            return ExitCode::from(1);
        }
    };
    match ggfm_protocol::read_struct(&raw) {
        Ok(fields) => {
            if env::args().any(|argument| argument == "--login-summary") {
                print_login_summary(&fields);
            } else if env::args().any(|argument| argument == "--master-summary") {
                print_master_summary(&fields);
            } else {
                println!("{fields:#?}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("could not decode {}: {error}", path.to_string_lossy());
            ExitCode::from(1)
        }
    }
}

fn print_master_summary(fields: &[Field]) {
    let Some(Value::Map { entries, .. }) = field(fields, 5) else {
        eprintln!("packet has no master data map at field 5");
        return;
    };
    for (key, value) in entries {
        println!("Master key: {key:?}");
        if let Value::Struct(tables) = value {
            for table in tables {
                if let Value::List { values, .. } = &table.value {
                    println!("  table {}: {} rows", table.id, values.len());
                    if table.id == 37 {
                        for row in values.iter().take(3) {
                            println!("    {row:?}");
                        }
                    }
                }
            }
        }
    }
}

fn print_login_summary(fields: &[Field]) {
    let Some(Value::Struct(data)) = field(fields, 5) else {
        eprintln!("packet has no userLogin Data struct at field 5");
        return;
    };
    let Some(Value::Struct(contents)) = field(data, 3) else {
        eprintln!("userLogin Data has no User_contents struct at field 3");
        return;
    };
    for entry in contents {
        match &entry.value {
            Value::List { values, .. } => {
                println!("User_contents field {}: {} row(s)", entry.id, values.len());
                for (index, value) in values.iter().enumerate() {
                    if let Value::Struct(row) = value {
                        let scalar_fields = row
                            .iter()
                            .filter_map(|field| {
                                scalar(field).map(|value| format!("{}={value}", field.id))
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("  [{index}] {scalar_fields}");
                    }
                }
            }
            other => println!("User_contents field {}: {other:?}", entry.id),
        }
    }
}

fn field(fields: &[Field], id: i16) -> Option<&Value> {
    fields
        .iter()
        .find(|field| field.id == id)
        .map(|field| &field.value)
}

fn scalar(field: &Field) -> Option<String> {
    match &field.value {
        Value::Bool(value) => Some(value.to_string()),
        Value::Byte(value) => Some(value.to_string()),
        Value::Double(value) => Some(value.to_string()),
        Value::I16(value) => Some(value.to_string()),
        Value::I32(value) => Some(value.to_string()),
        Value::I64(value) => Some(value.to_string()),
        Value::String(value) => Some(String::from_utf8_lossy(value).into_owned()),
        _ => None,
    }
}
