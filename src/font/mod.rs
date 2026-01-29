use crate::error::{BootError, Result};
use std::fs;
use std::path::Path;

/// Font information for console display
#[derive(Debug, Clone)]
pub struct FontInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub stride: usize,
}

/// Font loader for console fonts
pub struct FontLoader {
    font_dir: String,
}

impl FontLoader {
    pub fn new() -> Self {
        Self {
            font_dir: "/usr/share/consolefonts".to_string(),
        }
    }

    pub fn with_font_dir(font_dir: &str) -> Self {
        Self {
            font_dir: font_dir.to_string(),
        }
    }

    /// List available fonts
    pub fn list_fonts(&self) -> Result<Vec<String>> {
        let mut fonts = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.font_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext == "psf" || ext == "psfu" || ext == "cp" {
                            if let Some(name) = path.file_name() {
                                fonts.push(name.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
        }

        Ok(fonts)
    }

    /// Load PSF (PC Screen Font) format
    pub fn load_psf_font(&self, font_name: &str) -> Result<FontInfo> {
        let font_path = Path::new(&self.font_dir).join(font_name);
        let data = fs::read(&font_path)
            .map_err(|e| BootError::System(format!("Failed to read font file: {}", e)))?;

        if data.len() < 32 {
            return Err(BootError::System("Font file too small".to_string()));
        }

        // Check PSF magic
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let is_psf2 = magic == 0x864ab572; // PSF2 magic

        if is_psf2 {
            self.parse_psf2(&data, font_name)
        } else {
            // Try PSF1
            self.parse_psf1(&data, font_name)
        }
    }

    /// Parse PSF1 format
    fn parse_psf1(&self, data: &[u8], font_name: &str) -> Result<FontInfo> {
        if data.len() < 4 {
            return Err(BootError::System("PSF1 font too small".to_string()));
        }

        let _mode = data[2];
        let charsize = data[3] as u32;

        // Determine width and height
        let width = 8; // PSF1 is always 8 pixels wide
        let height = charsize;

        // Calculate data offset
        let header_size = 4;
        let glyph_count = 256; // Standard PSF1 has 256 glyphs
        let expected_size = header_size + (glyph_count * charsize) as usize;

        if data.len() < expected_size {
            return Err(BootError::System("PSF1 font truncated".to_string()));
        }

        let font_data = data[header_size..].to_vec();

        Ok(FontInfo {
            name: font_name.to_string(),
            width,
            height,
            data: font_data,
            stride: (width / 8) as usize,
        })
    }

    /// Parse PSF2 format
    fn parse_psf2(&self, data: &[u8], font_name: &str) -> Result<FontInfo> {
        if data.len() < 32 {
            return Err(BootError::System("PSF2 font too small".to_string()));
        }

        let version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        if version != 0 {
            return Err(BootError::System(format!(
                "Unsupported PSF2 version: {}",
                version
            )));
        }

        let header_size = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        let flags = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
        let glyph_count = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
        let bytes_per_glyph = u32::from_le_bytes([data[20], data[21], data[22], data[23]]);
        let height = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let width = u32::from_le_bytes([data[28], data[29], data[30], data[31]]);

        // Calculate data offset
        let header_size_usize = header_size as usize;
        let expected_size = header_size_usize + (glyph_count * bytes_per_glyph) as usize;

        if data.len() < expected_size {
            return Err(BootError::System("PSF2 font truncated".to_string()));
        }

        let font_data = data[header_size_usize..].to_vec();
        let has_unicode_table = (flags & 0x01) != 0;

        // If there's a Unicode table, we might need to handle it
        if has_unicode_table {
            // Unicode table starts after glyph data
            let unicode_table_offset = header_size_usize + (glyph_count * bytes_per_glyph) as usize;
            if data.len() > unicode_table_offset {
                // Unicode table present, but we don't parse it for now
                // println!("Font has Unicode table ({} bytes)", data.len() - unicode_table_offset);
            }
        }

        Ok(FontInfo {
            name: font_name.to_string(),
            width,
            height,
            data: font_data,
            stride: ((width + 7) / 8) as usize, // Round up to nearest byte
        })
    }

    /// Load default font (try to find a suitable one)
    pub fn load_default_font(&self) -> Result<FontInfo> {
        let fonts = self.list_fonts()?;

        // Try to find a good default font
        let preferred_fonts = [
            "ter-powerline-v32n.psf.gz",
            "ter-v32n.psf.gz",
            "default8x16.psf",
            "lat9w-16.psfu",
            "lat2-16.psfu",
        ];

        for preferred in &preferred_fonts {
            if fonts.contains(&preferred.to_string()) {
                return self.load_psf_font(preferred);
            }
        }

        // Try any PSF font
        for font in &fonts {
            if font.ends_with(".psf") || font.ends_with(".psfu") {
                return self.load_psf_font(font);
            }
        }

        // Create a simple built-in font as fallback
        self.create_fallback_font()
    }

    /// Create a simple 8x16 fallback font
    fn create_fallback_font(&self) -> Result<FontInfo> {
        // Simple 8x16 font (256 glyphs * 16 bytes each)
        let mut font_data = Vec::new();
        
        // Create simple block glyphs
        for _ in 0..256 {
            // Each glyph is 16 bytes (8x16 pixels)
            for row in 0..16 {
                if row < 8 {
                    // Top half: pattern
                    font_data.push(0xAA); // 10101010
                } else {
                    // Bottom half: inverted pattern
                    font_data.push(0x55); // 01010101
                }
            }
        }

        Ok(FontInfo {
            name: "builtin-8x16".to_string(),
            width: 8,
            height: 16,
            data: font_data,
            stride: 1, // 8 bits = 1 byte per row
        })
    }

    /// Convert font to raw bitmap format for bootloader
    pub fn to_raw_bitmap(&self, font_info: &FontInfo) -> Vec<u8> {
        // Simple conversion: just return the font data
        // In a real implementation, we might need to convert to a specific format
        font_info.data.clone()
    }

    /// Get font dimensions
    pub fn get_dimensions(&self, font_info: &FontInfo) -> (u32, u32) {
        (font_info.width, font_info.height)
    }

    /// Calculate font stride (bytes per row)
    pub fn get_stride(&self, font_info: &FontInfo) -> usize {
        font_info.stride
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_loader_creation() {
        let loader = FontLoader::new();
        assert_eq!(loader.font_dir, "/usr/share/consolefonts");
    }

    #[test]
    fn test_fallback_font() {
        let loader = FontLoader::new();
        let font = loader.create_fallback_font().unwrap();
        
        assert_eq!(font.name, "builtin-8x16");
        assert_eq!(font.width, 8);
        assert_eq!(font.height, 16);
        assert_eq!(font.data.len(), 256 * 16); // 256 glyphs * 16 bytes each
        assert_eq!(font.stride, 1);
    }

    #[test]
    fn test_font_dimensions() {
        let loader = FontLoader::new();
        let font = loader.create_fallback_font().unwrap();
        
        let (width, height) = loader.get_dimensions(&font);
        assert_eq!(width, 8);
        assert_eq!(height, 16);
        
        let stride = loader.get_stride(&font);
        assert_eq!(stride, 1);
    }
}
