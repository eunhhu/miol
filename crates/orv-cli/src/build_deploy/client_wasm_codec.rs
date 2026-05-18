use crate::fnv1a64;

pub(crate) const WASM_MODULE_HEADER: &[u8] = b"\0asm\x01\0\0\0";
pub(crate) const CLIENT_WASM_CUSTOM_SECTION_NAME: &str = "orv.client";
pub(crate) const CLIENT_WASM_START_EXPORT: &str = "orv_start";
pub(crate) const CLIENT_WASM_RENDER_PTR_EXPORT: &str = "orv_render_ptr";
pub(crate) const CLIENT_WASM_RENDER_LEN_EXPORT: &str = "orv_render_len";
pub(crate) const CLIENT_WASM_MEMORY_EXPORT: &str = "memory";

pub(crate) fn client_wasm_metadata_value_from_bytes(
    bytes: &[u8],
) -> anyhow::Result<serde_json::Value> {
    let payload = client_wasm_custom_section_payload(bytes)?
        .ok_or_else(|| anyhow::anyhow!("client_wasm bundle does not declare ORV metadata"))?;
    let payload = std::str::from_utf8(payload)
        .map_err(|e| anyhow::anyhow!("client_wasm ORV metadata is not UTF-8: {e}"))?;
    serde_json::from_str(payload)
        .map_err(|e| anyhow::anyhow!("client_wasm ORV metadata is not JSON: {e}"))
}

pub(crate) fn client_wasm_exports_function(bytes: &[u8], name: &str) -> anyhow::Result<bool> {
    Ok(client_wasm_export_index(bytes, name, 0)?.is_some())
}

pub(crate) fn client_wasm_export_function_index(
    bytes: &[u8],
    name: &str,
) -> anyhow::Result<Option<u32>> {
    client_wasm_export_index(bytes, name, 0)
}

pub(crate) fn client_wasm_export_index(
    bytes: &[u8],
    name: &str,
    expected_kind: u8,
) -> anyhow::Result<Option<u32>> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 7 {
            return wasm_export_section_index(bytes, offset, section_end, name, expected_kind);
        }
        offset = section_end;
    }
    Ok(None)
}

pub(crate) fn wasm_export_section_index(
    bytes: &[u8],
    mut offset: usize,
    section_end: usize,
    name: &str,
    expected_kind: u8,
) -> anyhow::Result<Option<u32>> {
    let export_count = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
    for _ in 0..export_count {
        let name_len = read_wasm_u32_leb(bytes, &mut offset, section_end)? as usize;
        let name_end = offset
            .checked_add(name_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid export name"))?;
        if name_end > section_end {
            anyhow::bail!("client_wasm bundle has invalid export name");
        }
        let export_name_matches = &bytes[offset..name_end] == name.as_bytes();
        offset = name_end;
        if offset >= section_end {
            anyhow::bail!("client_wasm bundle has truncated export descriptor");
        }
        let kind = bytes[offset];
        offset += 1;
        let index = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
        if export_name_matches && kind == expected_kind {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

pub(crate) fn client_wasm_custom_section_payload(bytes: &[u8]) -> anyhow::Result<Option<&[u8]>> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 0 {
            let mut section_offset = offset;
            let name_len = read_wasm_u32_leb(bytes, &mut section_offset, section_end)? as usize;
            let name_end = section_offset.checked_add(name_len).ok_or_else(|| {
                anyhow::anyhow!("client_wasm bundle has invalid custom section name")
            })?;
            if name_end > section_end {
                anyhow::bail!("client_wasm bundle has invalid custom section name");
            }
            if &bytes[section_offset..name_end] == CLIENT_WASM_CUSTOM_SECTION_NAME.as_bytes() {
                return Ok(Some(&bytes[name_end..section_end]));
            }
        }
        offset = section_end;
    }
    Ok(None)
}

pub(crate) fn verify_client_wasm_initial_render_data(
    bytes: &[u8],
    initial_render: &serde_json::Value,
) -> anyhow::Result<()> {
    let expected_len = initial_render
        .get("byte_length")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("client_wasm initial_render byte_length is required"))?;
    let expected_hash = initial_render
        .get("html_hash")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("client_wasm initial_render html_hash is required"))?;
    let data = client_wasm_initial_render_data(bytes)?.unwrap_or(&[]);
    let actual_len = u64::try_from(data.len())
        .map_err(|_| anyhow::anyhow!("client_wasm initial_render byte_length is invalid"))?;
    if actual_len != expected_len {
        anyhow::bail!("client_wasm initial_render byte_length mismatch");
    }
    let actual_hash = format!("{:016x}", fnv1a64(data));
    if actual_hash != expected_hash {
        anyhow::bail!("client_wasm initial_render html_hash mismatch");
    }
    let expected_len_i32 = i32::try_from(expected_len)
        .map_err(|_| anyhow::anyhow!("client_wasm initial_render byte_length exceeds wasm i32"))?;
    let ptr = client_wasm_exported_i32_const(bytes, CLIENT_WASM_RENDER_PTR_EXPORT)?
        .ok_or_else(|| anyhow::anyhow!("client_wasm orv_render_ptr export body is missing"))?;
    if ptr != 0 {
        anyhow::bail!("client_wasm orv_render_ptr export must return initial render pointer");
    }
    let len = client_wasm_exported_i32_const(bytes, CLIENT_WASM_RENDER_LEN_EXPORT)?
        .ok_or_else(|| anyhow::anyhow!("client_wasm orv_render_len export body is missing"))?;
    if len != expected_len_i32 {
        anyhow::bail!("client_wasm orv_render_len export must return initial render byte_length");
    }
    Ok(())
}

pub(crate) fn client_wasm_initial_render_data(bytes: &[u8]) -> anyhow::Result<Option<&[u8]>> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 11 {
            return wasm_initial_render_data_section(bytes, offset, section_end);
        }
        offset = section_end;
    }
    Ok(None)
}

pub(crate) fn wasm_initial_render_data_section(
    bytes: &[u8],
    mut offset: usize,
    section_end: usize,
) -> anyhow::Result<Option<&[u8]>> {
    let data_count = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
    for _ in 0..data_count {
        let flags = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
        if flags != 0 {
            anyhow::bail!("client_wasm initial_render data segment must target memory 0");
        }
        if offset >= section_end || bytes[offset] != 0x41 {
            anyhow::bail!("client_wasm initial_render data segment must use i32.const offset");
        }
        offset += 1;
        let memory_offset = read_wasm_i32_leb(bytes, &mut offset, section_end)?;
        if offset >= section_end || bytes[offset] != 0x0b {
            anyhow::bail!("client_wasm initial_render data segment offset is invalid");
        }
        offset += 1;
        let data_len = read_wasm_u32_leb(bytes, &mut offset, section_end)? as usize;
        let data_end = offset
            .checked_add(data_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm initial_render data segment is invalid"))?;
        if data_end > section_end {
            anyhow::bail!("client_wasm initial_render data segment is truncated");
        }
        if memory_offset == 0 {
            return Ok(Some(&bytes[offset..data_end]));
        }
        offset = data_end;
    }
    Ok(None)
}

pub(crate) fn client_wasm_exported_i32_const(
    bytes: &[u8],
    name: &str,
) -> anyhow::Result<Option<i32>> {
    let Some(function_index) = client_wasm_export_function_index(bytes, name)? else {
        return Ok(None);
    };
    let imported_function_count = client_wasm_imported_function_count(bytes)?;
    if function_index < imported_function_count {
        anyhow::bail!("client_wasm {name} export must not point at an imported function");
    }
    let code_index = function_index - imported_function_count;
    client_wasm_code_function_i32_const(bytes, code_index)
}

pub(crate) fn client_wasm_imported_function_count(bytes: &[u8]) -> anyhow::Result<u32> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 2 {
            anyhow::bail!("client_wasm render exports must not depend on imported functions");
        }
        offset = section_end;
    }
    Ok(0)
}

pub(crate) fn client_wasm_code_function_i32_const(
    bytes: &[u8],
    target_index: u32,
) -> anyhow::Result<Option<i32>> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 10 {
            return wasm_code_section_i32_const(bytes, offset, section_end, target_index);
        }
        offset = section_end;
    }
    Ok(None)
}

pub(crate) fn wasm_code_section_i32_const(
    bytes: &[u8],
    mut offset: usize,
    section_end: usize,
    target_index: u32,
) -> anyhow::Result<Option<i32>> {
    let function_count = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
    for index in 0..function_count {
        let body_len = read_wasm_u32_leb(bytes, &mut offset, section_end)? as usize;
        let body_end = offset
            .checked_add(body_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm function body length is invalid"))?;
        if body_end > section_end {
            anyhow::bail!("client_wasm function body is truncated");
        }
        if index == target_index {
            return wasm_i32_const_body(bytes, offset, body_end).map(Some);
        }
        offset = body_end;
    }
    Ok(None)
}

pub(crate) fn wasm_i32_const_body(
    bytes: &[u8],
    mut offset: usize,
    body_end: usize,
) -> anyhow::Result<i32> {
    let local_decl_count = read_wasm_u32_leb(bytes, &mut offset, body_end)?;
    if local_decl_count != 0 {
        anyhow::bail!("client_wasm render export body must not declare locals");
    }
    if offset >= body_end || bytes[offset] != 0x41 {
        anyhow::bail!("client_wasm render export body must return i32.const");
    }
    offset += 1;
    let value = read_wasm_i32_leb(bytes, &mut offset, body_end)?;
    if offset >= body_end || bytes[offset] != 0x0b {
        anyhow::bail!("client_wasm render export body must end after i32.const");
    }
    offset += 1;
    if offset != body_end {
        anyhow::bail!("client_wasm render export body has trailing instructions");
    }
    Ok(value)
}

pub(crate) fn read_wasm_u32_leb(
    bytes: &[u8],
    offset: &mut usize,
    limit: usize,
) -> anyhow::Result<u32> {
    let mut value = 0u32;
    let mut shift = 0;
    for _ in 0..5 {
        if *offset >= limit {
            anyhow::bail!("client_wasm bundle has truncated LEB128 length");
        }
        let byte = bytes[*offset];
        *offset += 1;
        if shift == 28 && (byte & 0xf0) != 0 {
            anyhow::bail!("client_wasm bundle has invalid u32 LEB128 length");
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    anyhow::bail!("client_wasm bundle has invalid u32 LEB128 length")
}

pub(crate) fn read_wasm_i32_leb(
    bytes: &[u8],
    offset: &mut usize,
    limit: usize,
) -> anyhow::Result<i32> {
    let mut value = 0i32;
    let mut shift = 0;
    for _ in 0..5 {
        if *offset >= limit {
            anyhow::bail!("client_wasm bundle has truncated i32 LEB128");
        }
        let byte = bytes[*offset];
        *offset += 1;
        value |= i32::from(byte & 0x7f) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 32 && (byte & 0x40) != 0 {
                value |= !0 << shift;
            }
            return Ok(value);
        }
    }
    anyhow::bail!("client_wasm bundle has invalid i32 LEB128")
}

pub(crate) fn wasm_min_pages(byte_len: usize) -> anyhow::Result<u32> {
    let pages = byte_len.div_ceil(65_536).max(1);
    u32::try_from(pages)
        .map_err(|_| anyhow::anyhow!("client initial render exceeds wasm32 memory page count"))
}

pub(crate) fn push_wasm_const_i32_function(out: &mut Vec<u8>, value: i32) {
    let mut body = Vec::new();
    push_wasm_u32_leb(&mut body, 0);
    body.push(0x41);
    push_wasm_i32_leb(&mut body, value);
    body.push(0x0b);
    push_wasm_len(out, body.len());
    out.extend(body);
}

pub(crate) fn push_wasm_section(out: &mut Vec<u8>, id: u8, section: &[u8]) {
    out.push(id);
    push_wasm_len(out, section.len());
    out.extend_from_slice(section);
}

pub(crate) fn push_wasm_len(out: &mut Vec<u8>, len: usize) {
    let len = u32::try_from(len).expect("WASM section length fits in u32");
    push_wasm_u32_leb(out, len);
}

pub(crate) fn push_wasm_u32_leb(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

pub(crate) fn push_wasm_i32_leb(out: &mut Vec<u8>, mut value: i32) {
    loop {
        let byte = (value as u8) & 0x7f;
        value >>= 7;
        let done = (value == 0 && (byte & 0x40) == 0) || (value == -1 && (byte & 0x40) != 0);
        if done {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}
