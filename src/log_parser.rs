use crate::models::DiagnosticItem;

pub fn parse_log(output: &str) -> (Vec<DiagnosticItem>, Vec<DiagnosticItem>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // error: FILE:LINE: MESSAGE
        if let Some(rest) = trimmed.strip_prefix("error: ") {
            let item = parse_file_line_message(rest);
            errors.push(item);
            continue;
        }

        // warning: MESSAGE
        if let Some(rest) = trimmed.strip_prefix("warning: ") {
            let item = parse_file_line_message(rest);
            warnings.push(item);
            continue;
        }

        // LaTeX Warning: ...
        if let Some(rest) = trimmed.strip_prefix("LaTeX Warning: ") {
            warnings.push(DiagnosticItem {
                file: None,
                line: None,
                message: rest.to_string(),
            });
            continue;
        }

        // LaTeX Error: ...
        if let Some(rest) = trimmed.strip_prefix("LaTeX Error: ") {
            errors.push(DiagnosticItem {
                file: None,
                line: None,
                message: rest.to_string(),
            });
            continue;
        }

        // Overfull/Underfull hbox/vbox
        if trimmed.starts_with("Overfull \\hbox")
            || trimmed.starts_with("Underfull \\hbox")
            || trimmed.starts_with("Overfull \\vbox")
            || trimmed.starts_with("Underfull \\vbox")
        {
            warnings.push(DiagnosticItem {
                file: None,
                line: None,
                message: trimmed.to_string(),
            });
            continue;
        }

        // ! LaTeX error lines
        if trimmed.starts_with('!') {
            errors.push(DiagnosticItem {
                file: None,
                line: None,
                message: trimmed.trim_start_matches('!').trim().to_string(),
            });
        }
    }

    (errors, warnings)
}

/// Tries to parse "FILE:LINE: MESSAGE" or "FILE:LINE:COL: MESSAGE", falls back to plain message
fn parse_file_line_message(s: &str) -> DiagnosticItem {
    // Try FILE:LINE: MESSAGE pattern
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() >= 3 {
        let file_candidate = parts[0].trim();
        let line_candidate = parts[1].trim();
        let message = parts[2..].join(":").trim().to_string();

        if let Ok(line_num) = line_candidate.parse::<u32>() {
            if !file_candidate.is_empty() && !message.is_empty() {
                return DiagnosticItem {
                    file: Some(file_candidate.to_string()),
                    line: Some(line_num),
                    message,
                };
            }
        }
    }

    DiagnosticItem {
        file: None,
        line: None,
        message: s.to_string(),
    }
}
