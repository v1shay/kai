//! Raster tool output travels in the transcript as a bounded inline preview.
//! Provider data URLs are never treated as URLs to fetch.
use base64::{Engine, engine::general_purpose::STANDARD};
use image::ImageEncoder;
use serde_json::Value;
use std::sync::Arc;

const PREFIX: &str = "kai-tool-image:";
// Leave room for the card envelope in the remote live transcript's 48 KiB tail.
const PREVIEW_BYTES: usize = 30 * 1024;
pub(crate) const MAX_SOURCE_TEXT: usize = 22 * 1024 * 1024;

pub(crate) fn is_target(target: &str) -> bool {
    preview_data(target).is_some()
}

pub(crate) fn preview_data(target: &str) -> Option<&str> {
    target.strip_prefix(PREFIX).filter(|data| !data.is_empty() && data.len() <= PREVIEW_BYTES.div_ceil(3) * 4)
}

pub(crate) fn preview(url: &str) -> Option<String> {
    let (mime, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    if !mime.starts_with("image/") || data.is_empty() || data.len() > MAX_SOURCE_TEXT {
        return None;
    }
    let bytes = STANDARD.decode(data).ok()?;
    if !crate::attachments::supported_content(&bytes) { return None; }
    let mut pixels = image::DynamicImage::ImageRgba8(crate::background::attachment_thumbnail(&bytes, 1152).ok()?);
    for side in [1152, 880, 660, 480, 320, 220] {
        pixels = pixels.resize(side, side, image::imageops::FilterType::Triangle);
        let mut encoded = Vec::new();
        image::codecs::png::PngEncoder::new(&mut encoded)
            .write_image(pixels.as_bytes(), pixels.width(), pixels.height(), image::ExtendedColorType::Rgba8)
            .ok()?;
        if encoded.len() <= PREVIEW_BYTES {
            return Some(format!("{PREFIX}{}", STANDARD.encode(encoded)));
        }
        let rgb = pixels.to_rgb8();
        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 82).encode_image(&rgb).ok()?;
        if encoded.len() <= PREVIEW_BYTES {
            return Some(format!("{PREFIX}{}", STANDARD.encode(encoded)));
        }
    }
    None
}

pub(crate) fn cards(result: &Value) -> impl Iterator<Item = (usize, crate::chat_interactions::ChatInteraction)> + '_ {
    use crate::chat_interactions::ChatInteraction;
    let blocks = result.as_array().or_else(|| result["content"].as_array());
    blocks.into_iter().flatten().enumerate().filter_map(|(index, block)| {
        let url = match block["type"].as_str()? {
            "input_image" | "output_image" | "image" | "imageUrl" | "image_url" => {
                block["image_url"].as_str().or_else(|| block["imageUrl"].as_str())
                    .or_else(|| block["url"].as_str()).map(std::borrow::Cow::Borrowed)
                    .or_else(|| {
                        let data = block["data"].as_str().filter(|data| data.len() <= MAX_SOURCE_TEXT)?;
                        let mime = block["mimeType"].as_str().or_else(|| block["mime_type"].as_str())?;
                        Some(format!("data:{mime};base64,{data}").into())
                    })
            }
            _ => None,
        }?;
        Some((index, ChatInteraction::ToolImage { target: preview(&url)? }))
    })
}

pub(crate) fn decode(target: &str) -> Result<Arc<gpui::RenderImage>, &'static str> {
    if !is_target(target) { return Err("Invalid inline image"); }
    let bytes = STANDARD.decode(&target[PREFIX.len()..]).map_err(|_| "Invalid inline image")?;
    let mut pixels = crate::background::attachment_thumbnail(&bytes, 768)
        .map_err(|_| "Could not decode the tool image")?;
    for pixel in pixels.pixels_mut() { pixel.0.swap(0, 2); }
    Ok(Arc::new(gpui::RenderImage::new(vec![image::Frame::new(pixels)])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{chat_interactions::ChatInteraction, presentation::MessageBlock};
    use serde_json::json;

    #[test]
    fn inline_tool_outputs_share_the_transcript_renderer() {
        for color in [[17, 91, 204, 255], [255, 255, 255, 255]] {
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(&color.repeat(48), 8, 6, image::ExtendedColorType::Rgba8)
                .unwrap();
            let encoded = STANDARD.encode(png);
            let native = json!([{"type":"input_text","text":"Capture complete"},
                {"type":"input_image","image_url":format!("data:image/png;base64,{encoded}")}]);
            let mcp = json!({"content":[{"type":"image","mimeType":"image/png","data":encoded}]});
            for result in [native, mcp] {
                let (_, card) = cards(&result).next().expect("typed image");
                let ChatInteraction::ToolImage { target } = &card else { panic!("expected tool image") };
                let bytes = STANDARD.decode(preview_data(target).unwrap()).unwrap();
                assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgba8().get_pixel(0, 0).0, color);
                assert!(decode(target).is_ok());
                let saved = crate::chat_interactions::serialize_interaction(&card);
                assert!(matches!(&crate::presentation::blocks(&saved)[..],
                    [MessageBlock::Attachment { target: inline, .. }] if inline == target));
            }
        }
        assert!(preview("https://example.invalid/screenshot.png").is_none());
        assert!(preview("data:image/png;base64,AAAA").is_none());
    }
}
