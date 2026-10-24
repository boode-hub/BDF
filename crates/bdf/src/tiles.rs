//! Page images on the GPU and the bookkeeping of what to ask the engine for.
//!
//! Each frame, views call [`TileCache::want`] for every image they would like
//! to draw. At the end of the frame [`TileCache::end_frame`] tells the engine
//! about new wants and, if the wanted set changed, which queued renders to
//! drop. Textures live in a byte-bounded LRU; anything drawn this frame is
//! never evicted.

use bdf_core::cache::ByteLru;
use bdf_core::engine::{DocId, Image, TileKey};
use bdf_core::worker::{Request, WorkerHandle};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

pub struct TileCache {
    lru: ByteLru<TileKey, TextureHandle>,
    in_flight: HashMap<TileKey, Instant>,
    wanted: Vec<(TileKey, u32)>,
    wanted_set: HashSet<TileKey>,
    last_hash: u64,
    pub uploads: u64,
}

impl TileCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            lru: ByteLru::new(budget_bytes),
            in_flight: HashMap::new(),
            wanted: Vec::new(),
            wanted_set: HashSet::new(),
            last_hash: 0,
            uploads: 0,
        }
    }

    pub fn set_budget(&mut self, bytes: usize) {
        self.lru.set_budget(bytes);
    }

    pub fn used_bytes(&self) -> usize {
        self.lru.used_bytes()
    }

    pub fn len(&self) -> usize {
        self.lru.len()
    }

    pub fn pending(&self) -> usize {
        self.in_flight.len()
    }

    /// The images still awaited (for diagnostics).
    pub fn pending_keys(&self) -> Vec<TileKey> {
        self.in_flight.keys().copied().collect()
    }

    pub fn begin_frame(&mut self) {
        self.lru.begin_frame();
        self.wanted.clear();
        self.wanted_set.clear();
    }

    /// Asks for an image; returns it when it is ready. Lower priority
    /// numbers render first.
    pub fn want(&mut self, key: TileKey, priority: u32) -> Option<TextureHandle> {
        if let Some(t) = self.lru.get(&key) {
            return Some(t.clone());
        }
        if self.wanted_set.insert(key) {
            self.wanted.push((key, priority));
        }
        None
    }

    /// Returns an image only if it is already cached (fallback drawing).
    pub fn cached(&mut self, key: &TileKey) -> Option<TextureHandle> {
        self.lru.get(key).cloned()
    }

    pub fn end_frame(&mut self, worker: &WorkerHandle) {
        let mut h = DefaultHasher::new();
        for (k, p) in &self.wanted {
            k.hash(&mut h);
            p.hash(&mut h);
        }
        let hash = h.finish();
        let now = Instant::now();
        if hash != self.last_hash {
            self.last_hash = hash;
            // Keep queued work we still want, drop the rest.
            worker.send(Request::Retain {
                keys: self.wanted.clone(),
            });
            let wanted = &self.wanted_set;
            self.in_flight.retain(|k, _| wanted.contains(k));
        }
        for &(key, priority) in &self.wanted {
            let stale = self
                .in_flight
                .get(&key)
                .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(20));
            if !self.in_flight.contains_key(&key) || stale {
                self.in_flight.insert(key, now);
                worker.send(Request::Render { key, priority });
            }
        }
    }

    /// Stores a rendered image as a texture.
    pub fn insert(&mut self, ctx: &egui::Context, key: TileKey, img: Image) {
        self.in_flight.remove(&key);
        if img.width == 0 || img.height == 0 {
            return;
        }
        let bytes = img.rgba.len();
        let color = ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.rgba,
        );
        let tex = ctx.load_texture(format!("tile-{key:?}"), color, TextureOptions::LINEAR);
        self.uploads += 1;
        self.lru.insert(key, tex, bytes);
    }

    pub fn failed(&mut self, key: &TileKey) {
        // Keep it "in flight" for a while so a broken page is not re-rendered every frame.
        self.in_flight.insert(*key, Instant::now());
    }

    pub fn forget_doc(&mut self, doc: DocId) {
        self.lru.retain(|k| k.doc != doc);
        self.in_flight.retain(|k, _| k.doc != doc);
    }
}
