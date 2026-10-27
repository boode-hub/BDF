//! State of one open document, shared by every tab that shows it.

use bdf_core::engine::{DocId, DocInfo, NewAnnot, PageData};
use bdf_core::text::SearchOptions;
use bdf_core::worker::{Request, SearchHit, WorkerHandle};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

/// Page data kept per document (text, links, annotations).
const PAGE_DATA_CACHE: usize = 96;

pub struct SearchState {
    pub query: String,
    pub opts: SearchOptions,
    pub ticket: u64,
    /// Sorted by page, then by order on the page.
    pub hits: Vec<SearchHit>,
    pub current: Option<usize>,
    pub searched: u32,
    pub total: u32,
    pub done: bool,
}

impl SearchState {
    /// The number of hits, with a "+" once the search stopped at its limit.
    pub fn count_label(&self) -> String {
        let n = self.hits.len();
        if n >= bdf_core::worker::MAX_SEARCH_HITS {
            format!("{n}+")
        } else {
            n.to_string()
        }
    }

    pub fn add_hits(&mut self, hits: Vec<SearchHit>) {
        let Some(page) = hits.first().map(|h| h.page) else {
            return;
        };
        let at = self.hits.partition_point(|h| h.page < page);
        // The current hit index shifts if hits land before it.
        if let Some(c) = self.current.as_mut()
            && *c >= at
        {
            *c += hits.len();
        }
        self.hits.splice(at..at, hits);
    }

    /// First hit on or after `page` (wrapping), used to start navigation
    /// from where the reader is.
    pub fn first_from(&self, page: u32) -> Option<usize> {
        if self.hits.is_empty() {
            return None;
        }
        let i = self.hits.partition_point(|h| h.page < page);
        Some(if i < self.hits.len() { i } else { 0 })
    }

    pub fn step(&mut self, forward: bool, from_page: u32) -> Option<usize> {
        let n = self.hits.len();
        if n == 0 {
            return None;
        }
        let next = match self.current {
            None => self.first_from(from_page).unwrap_or(0),
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
        };
        self.current = Some(next);
        Some(next)
    }

    /// Indices of hits on `page`.
    pub fn on_page(&self, page: u32) -> std::ops::Range<usize> {
        let a = self.hits.partition_point(|h| h.page < page);
        let b = self.hits.partition_point(|h| h.page <= page);
        a..b
    }
}

/// An annotation this session added, so it can be undone.
#[derive(Clone)]
pub struct AddedAnnot {
    pub page: u32,
    pub name: String,
    pub annot: NewAnnot,
}

/// Every annotation in the document, gathered page by page for the sidebar.
#[derive(Default)]
pub struct AnnotIndex {
    pub ticket: u64,
    pub pages_done: u32,
    pub complete: bool,
    /// A page changed since the scan started.
    pub stale: bool,
    pub items: Vec<(u32, bdf_core::engine::AnnotInfo)>,
}

pub struct DocState {
    pub id: DocId,
    pub info: Arc<DocInfo>,
    pub path: PathBuf,
    pub dirty: bool,
    pub saving: bool,
    /// Bumped on every edit of a page, so stale images are never shown.
    generations: HashMap<u32, u32>,
    page_data: HashMap<u32, (u32, Arc<PageData>)>,
    page_data_order: VecDeque<u32>,
    requested: HashSet<(u32, u32)>,
    pub search: Option<SearchState>,
    pub undo: Vec<AddedAnnot>,
    pub redo: Vec<AddedAnnot>,
    pub annots: AnnotIndex,
}

impl DocState {
    pub fn new(id: DocId, info: DocInfo) -> Self {
        let path = info.path.clone();
        Self {
            id,
            info: Arc::new(info),
            path,
            dirty: false,
            saving: false,
            generations: HashMap::new(),
            page_data: HashMap::new(),
            page_data_order: VecDeque::new(),
            requested: HashSet::new(),
            search: None,
            undo: Vec::new(),
            redo: Vec::new(),
            annots: AnnotIndex::default(),
        }
    }

    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn page_count(&self) -> usize {
        self.info.page_sizes.len()
    }

    pub fn generation(&self, page: u32) -> u32 {
        self.generations.get(&page).copied().unwrap_or(0)
    }

    /// Marks a page as changed: its images and data are refetched.
    pub fn bump(&mut self, page: u32) {
        *self.generations.entry(page).or_insert(0) += 1;
        self.page_data.remove(&page);
        self.requested.retain(|(p, _)| *p != page);
        self.annots.stale = true;
    }

    /// Page data if loaded; otherwise asks the engine for it.
    pub fn page_data(&mut self, page: u32, worker: &WorkerHandle) -> Option<Arc<PageData>> {
        let gen_ = self.generation(page);
        if let Some((g, d)) = self.page_data.get(&page)
            && *g == gen_
        {
            return Some(d.clone());
        }
        if self.requested.insert((page, gen_)) {
            worker.send(Request::PageData { doc: self.id, page });
        }
        None
    }

    pub fn peek_page_data(&self, page: u32) -> Option<Arc<PageData>> {
        let g = self.generation(page);
        self.page_data
            .get(&page)
            .filter(|(pg, _)| *pg == g)
            .map(|(_, d)| d.clone())
    }

    pub fn store_page_data(&mut self, page: u32, data: PageData) {
        let g = self.generation(page);
        self.requested.remove(&(page, g));
        self.page_data.insert(page, (g, Arc::new(data)));
        self.page_data_order.retain(|p| *p != page);
        self.page_data_order.push_back(page);
        while self.page_data_order.len() > PAGE_DATA_CACHE {
            if let Some(old) = self.page_data_order.pop_front() {
                self.page_data.remove(&old);
            }
        }
    }

    /// Forgets a failed request so it can be retried later.
    pub fn page_data_failed(&mut self, page: u32) {
        let g = self.generation(page);
        self.requested.remove(&(page, g));
    }

    pub fn label(&self, page: usize) -> String {
        match self.info.page_labels.get(page).and_then(|l| l.clone()) {
            Some(l) => format!("{l} ({} of {})", page + 1, self.page_count()),
            None => format!("{} of {}", page + 1, self.page_count()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(page: u32) -> SearchHit {
        SearchHit {
            page,
            rects: vec![],
            snippet: String::new(),
            snippet_match: 0..0,
        }
    }

    #[test]
    fn hits_stay_sorted_and_current_tracks() {
        let mut s = SearchState {
            query: "x".into(),
            opts: Default::default(),
            ticket: 1,
            hits: vec![],
            current: None,
            searched: 0,
            total: 10,
            done: false,
        };
        s.add_hits(vec![hit(5), hit(5)]);
        s.add_hits(vec![hit(8)]);
        assert_eq!(s.step(true, 6), Some(2)); // first hit at/after page 6
        s.add_hits(vec![hit(1)]); // arrives after wrapping
        assert_eq!(s.current, Some(3));
        assert_eq!(
            s.hits.iter().map(|h| h.page).collect::<Vec<_>>(),
            vec![1, 5, 5, 8]
        );
        assert_eq!(s.step(true, 0), Some(0)); // wraps
        assert_eq!(s.step(false, 0), Some(3));
        assert_eq!(s.on_page(5), 1..3);
        assert_eq!(s.on_page(6), 3..3);
        assert_eq!(s.first_from(9), Some(0));
    }
}
