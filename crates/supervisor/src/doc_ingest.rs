use anyhow::{bail, Context, Result};
use calamine::{open_workbook_auto, Data, Reader};
use quick_xml::{events::Event, Reader as XmlReader};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
};
use zip::ZipArchive;

pub fn extract_document_to_markdown(path: &Path) -> Result<(String, String)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let markdown = match ext.as_str() {
        "md" | "txt" | "csv" | "json" | "log" | "yaml" | "yml" | "toml" => {
            std::fs::read_to_string(path)
                .with_context(|| format!("Failed reading UTF-8 text file {}", path.display()))?
        }
        "pdf" => extract_pdf_with_ocr_fallback(path)?,
        "docx" => extract_docx_to_markdown(path)?,
        "xlsx" | "xls" | "ods" => extract_spreadsheet_to_markdown(path)?,
        "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp" => extract_image_ocr(path)?,
        other => bail!(
            "Unsupported extension '.{}' (supported: pdf, docx, xlsx, xls, ods, csv, md, txt, png, jpg, webp, tiff)",
            other
        ),
    };

    let cleaned = normalize_whitespace(&markdown);
    if cleaned.trim().is_empty() {
        bail!("No extractable text found in {}", path.display());
    }

    Ok((ext, cleaned))
}

fn extract_pdf_with_ocr_fallback(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed reading PDF {}", path.display()))?;
    let native_text = pdf_extract::extract_text_from_mem(&bytes).unwrap_or_default();
    let trimmed = native_text.trim();

    if trimmed.chars().filter(|c| !c.is_whitespace()).count() >= 32 {
        return Ok(native_text);
    }

    let tmp_dir = std::env::temp_dir().join(format!("harts-ocr-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir)?;
    let prefix = tmp_dir.join("page");

    let ppm_status = Command::new("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg("200")
        .arg(path)
        .arg(&prefix)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let mut ocr_pages = Vec::new();
    if let Ok(st) = ppm_status {
        if st.success() {
            let mut entries: Vec<_> = std::fs::read_dir(&tmp_dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
                .collect();
            entries.sort();

            for (idx, page_png) in entries.iter().enumerate() {
                if let Ok(page_txt) = run_tesseract_on_file(page_png) {
                    let page_trimmed = page_txt.trim();
                    if !page_trimmed.is_empty() {
                        ocr_pages.push(format!("## Page {}\n\n{}", idx + 1, page_trimmed));
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);

    if !ocr_pages.is_empty() {
        return Ok(ocr_pages.join("\n\n"));
    }

    if !trimmed.is_empty() {
        Ok(native_text)
    } else {
        bail!(
            "PDF {} contains no selectable text layer and offline OCR (pdftoppm + tesseract) produced no text",
            path.display()
        )
    }
}

fn extract_image_ocr(path: &Path) -> Result<String> {
    let text = run_tesseract_on_file(path)
        .with_context(|| format!("Offline Tesseract OCR failed for {}", path.display()))?;
    Ok(text)
}

fn run_tesseract_on_file(image_path: &Path) -> Result<String> {
    let img_bytes = std::fs::read(image_path)?;
    let mut child = Command::new("tesseract")
        .arg("stdin")
        .arg("stdout")
        .arg("--oem")
        .arg("1")
        .arg("-l")
        .arg("eng")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to execute offline tesseract binary (ensure tesseract-ocr is installed)")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&img_bytes)?;
    }

    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("tesseract exited with status {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn extract_docx_to_markdown(path: &Path) -> Result<String> {
    let file = File::open(path)
        .with_context(|| format!("Failed opening DOCX {}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("Invalid DOCX archive {}", path.display()))?;

    let mut xml_bytes = Vec::new();
    {
        let mut doc_xml = archive
            .by_name("word/document.xml")
            .context("DOCX missing word/document.xml")?;
        doc_xml.read_to_end(&mut xml_bytes)?;
    }

    let mut reader = XmlReader::from_reader(xml_bytes.as_slice());
    reader.config_mut().trim_text(false);

    let mut out = String::new();
    let mut current_para = String::new();
    let mut in_paragraph = false;
    let mut in_text = false;
    let mut is_heading = false;
    let mut row_cells: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut in_cell = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                b"p" => {
                    in_paragraph = true;
                    current_para.clear();
                    is_heading = false;
                }
                b"t" => in_text = true,
                b"tr" => row_cells.clear(),
                b"tc" => {
                    in_cell = true;
                    current_cell.clear();
                }
                b"pStyle" => {
                    for attr in e.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"val" {
                            let val = String::from_utf8_lossy(&attr.value);
                            if val.starts_with("Heading") || val == "Title" {
                                is_heading = true;
                            }
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match e.local_name().as_ref() {
                b"pStyle" => {
                    for attr in e.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"val" {
                            let val = String::from_utf8_lossy(&attr.value);
                            if val.starts_with("Heading") || val == "Title" {
                                is_heading = true;
                            }
                        }
                    }
                }
                b"tab" => {
                    if in_cell {
                        current_cell.push(' ');
                    } else if in_paragraph {
                        current_para.push(' ');
                    }
                }
                b"br" => {
                    if in_cell {
                        current_cell.push(' ');
                    } else if in_paragraph {
                        current_para.push('\n');
                    }
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_text {
                    let txt = e.unescape().unwrap_or_default();
                    if in_cell {
                        current_cell.push_str(&txt);
                    } else if in_paragraph {
                        current_para.push_str(&txt);
                    }
                }
            }
            Ok(Event::End(ref e)) => match e.local_name().as_ref() {
                b"t" => in_text = false,
                b"p" => {
                    in_paragraph = false;
                    if !in_cell {
                        let trimmed = current_para.trim();
                        if !trimmed.is_empty() {
                            if is_heading {
                                out.push_str("## ");
                            }
                            out.push_str(trimmed);
                            out.push_str("\n\n");
                        }
                    }
                }
                b"tc" => {
                    in_cell = false;
                    row_cells.push(current_cell.trim().replace('|', "\\|"));
                }
                b"tr" => {
                    if !row_cells.is_empty() {
                        out.push_str("| ");
                        out.push_str(&row_cells.join(" | "));
                        out.push_str(" |\n");
                    }
                }
                b"tbl" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => bail!("XML parse error in DOCX {}: {}", path.display(), e),
            _ => {}
        }
    }

    Ok(out)
}

fn extract_spreadsheet_to_markdown(path: &Path) -> Result<String> {
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("Failed opening spreadsheet {}", path.display()))?;

    let mut out = String::new();
    let sheet_names = workbook.sheet_names().to_vec();

    for sheet in sheet_names {
        if let Ok(range) = workbook.worksheet_range(&sheet) {
            if range.is_empty() {
                continue;
            }
            out.push_str(&format!("## Sheet: {}\n\n", sheet));
            let mut row_idx = 0usize;
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|cell| match cell {
                        Data::Empty => String::new(),
                        Data::String(s) => s.trim().replace('|', "\\|"),
                        Data::Float(f) => format!("{}", f),
                        Data::Int(i) => format!("{}", i),
                        Data::Bool(b) => format!("{}", b),
                        Data::DateTime(dt) => format!("{}", dt),
                        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
                        Data::Error(e) => format!("#ERR:{:?}", e),
                    })
                    .collect();

                if cells.iter().all(|c| c.is_empty()) {
                    continue;
                }

                out.push_str("| ");
                out.push_str(&cells.join(" | "));
                out.push_str(" |\n");

                if row_idx == 0 {
                    let sep: Vec<&str> = cells.iter().map(|_| "---").collect();
                    out.push_str("| ");
                    out.push_str(&sep.join(" | "));
                    out.push_str(" |\n");
                }
                row_idx += 1;
            }
            out.push('\n');
        }
    }

    Ok(out)
}

fn normalize_whitespace(input: &str) -> String {
    let mut lines = Vec::new();
    let mut blank_run = 0usize;
    for raw_line in input.lines() {
        let trimmed = raw_line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 2 {
                lines.push(String::new());
            }
        } else {
            blank_run = 0;
            lines.push(trimmed.to_string());
        }
    }
    lines.join("\n")
}

pub fn chunk_markdown(markdown: &str, max_chars: usize, overlap_chars: usize) -> Vec<String> {
    let chars: Vec<char> = markdown.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    if chars.len() <= max_chars {
        return vec![markdown.trim().to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    let step = max_chars.saturating_sub(overlap_chars).max(128);

    while start < chars.len() {
        let end = (start + max_chars).min(chars.len());
        let slice: String = chars[start..end].iter().collect();
        let trimmed = slice.trim();
        if !trimmed.is_empty() {
            chunks.push(trimmed.to_string());
        }
        if end == chars.len() {
            break;
        }
        start += step;
    }

    chunks
}
