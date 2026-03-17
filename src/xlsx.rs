use quick_xml::escape::unescape;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

fn text_to_string(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    unescape(&raw).map(|c| c.into_owned()).unwrap_or_else(|_| raw.into_owned())
}

fn resolve_entity(name: &[u8]) -> &'static str {
    match name {
        b"lt" => "<",
        b"gt" => ">",
        b"amp" => "&",
        b"quot" => "\"",
        b"apos" => "'",
        _ => "",
    }
}

pub struct Workbook {
    pub sheets: Vec<Sheet>,
}

pub struct Sheet {
    pub name: String,
    pub cells: Vec<RawCell>,
    pub shared_formulas: HashMap<String, SharedFormula>,
}

pub struct RawCell {
    pub coord: String,
    pub formula: Option<String>,
    pub value: Option<String>,
    pub cell_type: Option<String>,
}

pub struct SharedFormula {
    pub master_coord: String,
    pub formula: String,
    pub range: String,
}

pub fn load_xlsx(path: &Path) -> Result<Workbook, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let sheet_names = parse_workbook_sheet_names(&mut archive)?;
    let shared_strings = parse_shared_strings(&mut archive)?;

    let mut sheets = Vec::new();
    for (i, name) in sheet_names.into_iter().enumerate() {
        let sheet_path = format!("xl/worksheets/sheet{}.xml", i + 1);
        if let Ok(sheet) = parse_sheet(&mut archive, &sheet_path, &name, &shared_strings) {
            sheets.push(sheet);
        }
    }

    Ok(Workbook { sheets })
}

fn parse_workbook_sheet_names(
    archive: &mut zip::ZipArchive<std::fs::File>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut xml = String::new();
    archive.by_name("xl/workbook.xml")?.read_to_string(&mut xml)?;

    let mut reader = Reader::from_str(&xml);
    let mut names = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) if e.name().as_ref() == b"sheet" => {
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == b"name" {
                        names.push(String::from_utf8(attr.value.to_vec())?);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(names)
}

fn parse_shared_strings(
    archive: &mut zip::ZipArchive<std::fs::File>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut xml = String::new();
    match archive.by_name("xl/sharedStrings.xml") {
        Ok(mut f) => f.read_to_string(&mut xml)?,
        Err(_) => return Ok(Vec::new()),
    };

    let mut reader = Reader::from_str(&xml);
    let mut strings = Vec::new();
    let mut buf = Vec::new();
    let mut current = String::new();
    let mut in_si = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"si" => {
                in_si = true;
                current.clear();
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"si" => {
                in_si = false;
                strings.push(current.clone());
            }
            Ok(Event::Text(ref e)) if in_si => {
                current.push_str(&text_to_string(e.as_ref()));
            }
            Ok(Event::GeneralRef(ref e)) if in_si => {
                current.push_str(resolve_entity(e.as_ref()));
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(strings)
}

fn parse_sheet(
    archive: &mut zip::ZipArchive<std::fs::File>,
    sheet_path: &str,
    name: &str,
    shared_strings: &[String],
) -> Result<Sheet, Box<dyn std::error::Error>> {
    let mut xml = String::new();
    archive.by_name(sheet_path)?.read_to_string(&mut xml)?;

    let mut reader = Reader::from_str(&xml);
    let mut buf = Vec::new();

    let mut cells = Vec::new();
    let mut shared_formulas: HashMap<String, SharedFormula> = HashMap::new();

    let mut coord = String::new();
    let mut cell_type: Option<String> = None;
    let mut formula: Option<String> = None;
    let mut value: Option<String> = None;
    let mut in_cell = false;
    let mut in_formula = false;
    let mut in_value = false;
    let mut shared_si: Option<String> = None;
    let mut shared_ref: Option<String> = None;
    let mut is_shared_master = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"c" => {
                in_cell = true;
                coord.clear();
                cell_type = None;
                formula = None;
                value = None;
                shared_si = None;
                shared_ref = None;
                is_shared_master = false;

                for attr in e.attributes().flatten() {
                    match attr.key.as_ref() {
                        b"r" => coord = String::from_utf8(attr.value.to_vec())?,
                        b"t" => cell_type = Some(String::from_utf8(attr.value.to_vec())?),
                        _ => {}
                    }
                }
            }
            Ok(Event::Start(ref e)) if in_cell && e.name().as_ref() == b"f" => {
                in_formula = true;
                for attr in e.attributes().flatten() {
                    match attr.key.as_ref() {
                        b"t" if attr.value.as_ref() == b"shared" => {
                            is_shared_master = true;
                        }
                        b"si" => shared_si = Some(String::from_utf8(attr.value.to_vec())?),
                        b"ref" => shared_ref = Some(String::from_utf8(attr.value.to_vec())?),
                        _ => {}
                    }
                }
            }
            Ok(Event::Empty(ref e)) if in_cell && e.name().as_ref() == b"f" => {
                let mut si = None;
                let mut is_shared = false;
                for attr in e.attributes().flatten() {
                    match attr.key.as_ref() {
                        b"t" if attr.value.as_ref() == b"shared" => is_shared = true,
                        b"si" => si = Some(String::from_utf8(attr.value.to_vec())?),
                        _ => {}
                    }
                }
                if is_shared {
                    shared_si = si;
                }
            }
            Ok(Event::Text(ref e)) if in_formula => {
                formula.get_or_insert_with(String::new).push_str(&text_to_string(e.as_ref()));
            }
            Ok(Event::GeneralRef(ref e)) if in_formula => {
                formula.get_or_insert_with(String::new).push_str(resolve_entity(e.as_ref()));
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"f" => {
                in_formula = false;
                if is_shared_master {
                    if let (Some(si), Some(f)) = (&shared_si, &formula) {
                        shared_formulas.insert(
                            si.clone(),
                            SharedFormula {
                                master_coord: coord.clone(),
                                formula: f.clone(),
                                range: shared_ref.take().unwrap_or_default(),
                            },
                        );
                    }
                }
            }
            Ok(Event::Start(ref e)) if in_cell && e.name().as_ref() == b"v" => {
                in_value = true;
            }
            Ok(Event::Text(ref e)) if in_value => {
                value.get_or_insert_with(String::new).push_str(&text_to_string(e.as_ref()));
            }
            Ok(Event::GeneralRef(ref e)) if in_value => {
                value.get_or_insert_with(String::new).push_str(resolve_entity(e.as_ref()));
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"v" => {
                in_value = false;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"c" => {
                if in_cell {
                    // Expand shared formula for dependents
                    if formula.is_none() {
                        if let Some(si) = &shared_si {
                            if let Some(sf) = shared_formulas.get(si) {
                                let (master_col, master_row) = parse_coord(&sf.master_coord);
                                let (dep_col, dep_row) = parse_coord(&coord);
                                let row_off = dep_row as i32 - master_row as i32;
                                let col_off = dep_col as i32 - master_col as i32;
                                formula = Some(offset_formula(&sf.formula, row_off, col_off));
                            }
                        }
                    }

                    let resolved_value = resolve_value(&value, &cell_type, shared_strings);
                    cells.push(RawCell {
                        coord: coord.clone(),
                        formula: formula.take(),
                        value: resolved_value,
                        cell_type: cell_type.take(),
                    });
                    in_cell = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(Sheet {
        name: name.to_string(),
        cells,
        shared_formulas,
    })
}

fn parse_coord(coord: &str) -> (i32, i32) {
    let mut col = 0i32;
    let mut row_str = String::new();
    for ch in coord.chars() {
        if ch.is_ascii_alphabetic() {
            col = col * 26 + (ch.to_ascii_uppercase() as i32 - 'A' as i32 + 1);
        } else if ch.is_ascii_digit() {
            row_str.push(ch);
        }
    }
    let row: i32 = row_str.parse().unwrap_or(0);
    (col, row)
}

fn offset_formula(formula: &str, row_off: i32, col_off: i32) -> String {
    let mut result = String::new();
    let chars: Vec<char> = formula.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Skip $ (absolute ref markers) - they suppress offset
        if chars[i] == '$' {
            result.push('$');
            // Collect the absolute column or row
            i += 1;
            if i < chars.len() && chars[i].is_ascii_alphabetic() {
                // $COL - absolute column, don't offset
                while i < chars.len() && chars[i].is_ascii_alphabetic() {
                    result.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() && chars[i] == '$' {
                    result.push('$');
                    i += 1;
                    // $ROW - absolute row, don't offset
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        result.push(chars[i]);
                        i += 1;
                    }
                } else if i < chars.len() && chars[i].is_ascii_digit() {
                    // Relative row after absolute column
                    let row_start = i;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    let row: i32 = chars[row_start..i].iter().collect::<String>().parse().unwrap_or(0);
                    result.push_str(&(row + row_off).to_string());
                }
            } else if i < chars.len() && chars[i].is_ascii_digit() {
                // $ROW - absolute row (after relative column)
                while i < chars.len() && chars[i].is_ascii_digit() {
                    result.push(chars[i]);
                    i += 1;
                }
            }
            continue;
        }

        // Check for cell reference: letter(s) followed by digit(s)
        if chars[i].is_ascii_alphabetic() && !is_part_of_function(&chars, i) {
            let col_start = i;
            while i < chars.len() && chars[i].is_ascii_uppercase() {
                i += 1;
            }
            let col_len = i - col_start;

            if col_len > 0 && i < chars.len() && chars[i].is_ascii_digit() {
                // This is a cell reference - offset it
                let col_str: String = chars[col_start..col_start + col_len].iter().collect();
                let col_num = col_str.bytes().fold(0i32, |acc, b| acc * 26 + (b - b'A' + 1) as i32);
                let new_col = col_num + col_off;
                result.push_str(&col_num_to_str(new_col));

                let row_start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let row: i32 = chars[row_start..i].iter().collect::<String>().parse().unwrap_or(0);
                result.push_str(&(row + row_off).to_string());
            } else {
                // Not a cell ref, just letters (function name etc)
                for ch in &chars[col_start..i] {
                    result.push(*ch);
                }
            }
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

fn is_part_of_function(chars: &[char], pos: usize) -> bool {
    // Check if this alphabetic run is followed by '(' (function call)
    // or preceded by '_' or '.' (like _xlfn.FLOOR)
    if pos > 0 && (chars[pos - 1] == '.' || chars[pos - 1] == '_') {
        return true;
    }
    let mut i = pos;
    while i < chars.len() && (chars[i].is_ascii_alphabetic() || chars[i] == '.' || chars[i] == '_') {
        i += 1;
    }
    // Skip whitespace
    while i < chars.len() && chars[i].is_ascii_whitespace() {
        i += 1;
    }
    i < chars.len() && chars[i] == '('
}

fn col_num_to_str(mut n: i32) -> String {
    if n <= 0 { return "A".to_string(); }
    let mut result = Vec::new();
    while n > 0 {
        n -= 1;
        result.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    result.reverse();
    String::from_utf8(result).unwrap_or_default()
}

fn resolve_value(
    value: &Option<String>,
    cell_type: &Option<String>,
    shared_strings: &[String],
) -> Option<String> {
    let v = value.as_ref()?;
    match cell_type.as_deref() {
        Some("s") => {
            let idx: usize = v.parse().ok()?;
            shared_strings.get(idx).cloned()
        }
        _ => Some(v.clone()),
    }
}
