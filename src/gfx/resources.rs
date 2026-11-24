use super::geometry::{error, positive};
use glyphon::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use mlua::{Result, UserData, UserDataMethods};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    fs,
    io::Read,
    path::PathBuf,
    rc::{Rc, Weak},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const TEXT_CACHE_BYTES: usize = 16 * 1024 * 1024;
pub struct Pixels {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}
impl Pixels {
    pub fn new(width: u32, height: u32, bytes: Vec<u8>) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            bytes,
        }
    }
}
pub struct Font {
    pub size: f32,
    family: Option<String>,
    system: FontSystem,
    swash: SwashCache,
    cache: VecDeque<(String, u32, Rc<Pixels>, f32)>,
    cache_bytes: usize,
}
#[derive(Clone)]
pub struct FontHandle(pub Rc<RefCell<Option<Font>>>);
#[derive(Clone)]
pub struct ImageHandle(pub Rc<RefCell<Option<Rc<Pixels>>>>);
#[derive(Default)]
pub struct Resources {
    fonts: HashMap<(PathBuf, u32), Weak<RefCell<Option<Font>>>>,
    images: HashMap<PathBuf, Weak<RefCell<Option<Rc<Pixels>>>>>,
}
impl Resources {
    pub fn font(&mut self, path: Option<String>, size: f32) -> Result<FontHandle> {
        positive(size, "font size", 512.)?;
        self.fonts
            .retain(|_, weak| weak.upgrade().is_some_and(|r| r.borrow().is_some()));
        let path = path
            .map(fs::canonicalize)
            .transpose()
            .map_err(|e| error(format!("cannot load font: {e}")))?
            .unwrap_or_default();
        let key = (path.clone(), size.to_bits());
        if let Some(value) = self.fonts.get(&key).and_then(Weak::upgrade) {
            return Ok(FontHandle(value));
        }
        if self.fonts.len() >= 64 {
            return Err(error("at most 64 live fonts are supported"));
        }
        let mut system = FontSystem::new();
        let family = if path.as_os_str().is_empty() {
            None
        } else {
            let mut bytes = Vec::new();
            fs::File::open(&path)
                .and_then(|file| file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes))
                .map_err(|e| error(format!("cannot read font {}: {e}", path.display())))?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(error("font file exceeds 16 MiB"));
            }
            let before: Vec<_> = system.db().faces().map(|f| f.id).collect();
            system.db_mut().load_font_data(bytes);
            let family = system
                .db()
                .faces()
                .find(|f| !before.contains(&f.id))
                .and_then(|f| f.families.first())
                .map(|(name, _)| name.clone())
                .ok_or_else(|| error("font file contains no supported font face"))?;
            // Prefer the requested file over an installed face with the same
            // family name, while retaining other families for Unicode fallback.
            let duplicates: Vec<_> = system
                .db()
                .faces()
                .filter(|f| before.contains(&f.id) && f.families.iter().any(|(n, _)| n == &family))
                .map(|f| f.id)
                .collect();
            for id in duplicates {
                system.db_mut().remove_face(id);
            }
            Some(family)
        };
        if system.db().faces().next().is_none() {
            return Err(error(
                "no fonts available; load a TrueType/OpenType font file",
            ));
        }
        let result = Rc::new(RefCell::new(Some(Font {
            size,
            family,
            system,
            swash: SwashCache::new(),
            cache: VecDeque::new(),
            cache_bytes: 0,
        })));
        self.fonts.insert(key, Rc::downgrade(&result));
        Ok(FontHandle(result))
    }
    pub fn image(&mut self, path: String) -> Result<ImageHandle> {
        let path = fs::canonicalize(path).map_err(|e| error(format!("cannot load image: {e}")))?;
        self.images
            .retain(|_, weak| weak.upgrade().is_some_and(|r| r.borrow().is_some()));
        if let Some(value) = self.images.get(&path).and_then(Weak::upgrade) {
            return Ok(ImageHandle(value));
        }
        if self.images.len() >= 256 {
            return Err(error("at most 256 live images are supported"));
        }
        if fs::metadata(&path).map_err(|e| error(e.to_string()))?.len() > 64 * 1024 * 1024 {
            return Err(error("image file exceeds 64 MiB"));
        }
        let mut reader = image::ImageReader::open(&path)
            .map_err(|e| error(e.to_string()))?
            .with_guessed_format()
            .map_err(|e| error(e.to_string()))?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let decoder = reader
            .into_decoder()
            .map_err(|e| error(format!("cannot decode image {}: {e}", path.display())))?;
        let (width, height) = image::ImageDecoder::dimensions(&decoder);
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) * 4 > 64 * 1024 * 1024
        {
            return Err(error("image dimensions/decoded size exceed limits"));
        }
        let image = image::DynamicImage::from_decoder(decoder)
            .map_err(|e| error(format!("cannot decode image {}: {e}", path.display())))?
            .to_rgba8();
        if image.width() == 0 || image.height() == 0 || image.as_raw().len() > 64 * 1024 * 1024 {
            return Err(error("image dimensions/decoded size exceed limits"));
        }
        let pixels = Rc::new(Pixels::new(image.width(), image.height(), image.into_raw()));
        let result = Rc::new(RefCell::new(Some(pixels)));
        self.images.insert(path, Rc::downgrade(&result));
        Ok(ImageHandle(result))
    }
}
impl Font {
    fn buffer(&mut self, text: &str, scale: f32) -> Result<Buffer> {
        if text.len() > 65536 {
            return Err(error("text exceeds 64 KiB"));
        }
        positive(scale, "text scale", 16.)?;
        let size = self.size * scale;
        let mut buffer = Buffer::new(&mut self.system, Metrics::new(size, size * 1.3));
        buffer.set_size(None, None);
        buffer.set_wrap(Wrap::None);
        buffer.set_text(
            text,
            &Attrs::new().family(
                self.family
                    .as_deref()
                    .map(Family::Name)
                    .unwrap_or(Family::SansSerif),
            ),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.system, false);
        Ok(buffer)
    }
    pub fn measure(&mut self, text: &str) -> Result<(f32, f32)> {
        let buffer = self.buffer(text, 1.)?;
        let width = buffer.layout_runs().map(|r| r.line_w).fold(0f32, f32::max);
        let height = buffer
            .layout_runs()
            .map(|r| r.line_top + r.line_height)
            .fold(self.size * 1.3, f32::max);
        Ok((width, height))
    }
    pub fn raster(&mut self, text: &str, scale: f32) -> Result<(Rc<Pixels>, f32)> {
        if let Some(index) = self
            .cache
            .iter()
            .position(|(t, s, _, _)| t == text && *s == scale.to_bits())
        {
            let item = self
                .cache
                .remove(index)
                .ok_or_else(|| error("text cache failure"))?;
            let result = (item.2.clone(), item.3);
            self.cache.push_back(item);
            return Ok(result);
        }
        let mut buffer = self.buffer(text, scale)?;
        let pad = (self.size * scale).ceil();
        let width = (buffer.layout_runs().map(|r| r.line_w).fold(0f32, f32::max) + pad * 2.)
            .ceil()
            .max(1.) as u32;
        let height = (buffer
            .layout_runs()
            .map(|r| r.line_top + r.line_height)
            .fold(self.size * scale * 1.3, f32::max)
            + pad * 2.)
            .ceil()
            .max(1.) as u32;
        if width > 8192
            || height > 8192
            || u64::from(width) * u64::from(height) * 4 > TEXT_CACHE_BYTES as u64
        {
            return Err(error(
                "rendered text exceeds 8192 pixels / 16 MiB; draw smaller text runs",
            ));
        }
        let mut bytes = vec![0u8; (width * height * 4) as usize];
        #[allow(deprecated)]
        buffer.draw(
            &mut self.system,
            &mut self.swash,
            Color::rgb(255, 255, 255),
            |x, y, w, h, color| {
                for dy in 0..h {
                    for dx in 0..w {
                        let px = x + i32::try_from(dx).unwrap_or(i32::MAX) + pad as i32;
                        let py = y + i32::try_from(dy).unwrap_or(i32::MAX) + pad as i32;
                        if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                            continue;
                        }
                        let index = ((py as u32 * width + px as u32) * 4) as usize;
                        let [r, g, b, a] = color.as_rgba();
                        let alpha = a as f32 / 255.;
                        let old = bytes[index + 3] as f32 / 255.;
                        let combined = alpha + old * (1. - alpha);
                        if combined > 0. {
                            for (i, c) in [r, g, b].iter().enumerate() {
                                bytes[index + i] = ((*c as f32 * alpha
                                    + bytes[index + i] as f32 * old * (1. - alpha))
                                    / combined)
                                    .round()
                                    as u8;
                            }
                            bytes[index + 3] = (combined * 255.).round() as u8;
                        }
                    }
                }
            },
        );
        while self.cache_bytes + bytes.len() > TEXT_CACHE_BYTES || self.cache.len() >= 128 {
            if let Some((_, _, old, _)) = self.cache.pop_front() {
                self.cache_bytes -= old.bytes.len();
            } else {
                break;
            }
        }
        self.cache_bytes += bytes.len();
        let pixels = Rc::new(Pixels::new(width, height, bytes));
        self.cache.push_back((
            text.to_string(),
            scale.to_bits(),
            pixels.clone(),
            pad / scale,
        ));
        Ok((pixels, pad / scale))
    }
}
impl UserData for FontHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("dispose", |_, this, ()| {
            this.0.borrow_mut().take();
            Ok(())
        });
        m.add_method("isDisposed", |_, this, ()| Ok(this.0.borrow().is_none()));
    }
}
impl UserData for ImageHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("dispose", |_, this, ()| {
            this.0.borrow_mut().take();
            Ok(())
        });
        m.add_method("isDisposed", |_, this, ()| Ok(this.0.borrow().is_none()));
        m.add_method("getSize", |_, this, ()| {
            let r = this.0.borrow();
            let p = r.as_ref().ok_or_else(|| error("image is disposed"))?;
            Ok((p.width, p.height))
        });
    }
}
