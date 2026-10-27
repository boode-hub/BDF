//! The left sidebar: thumbnails, outline, bookmarks, annotations, search results.

use crate::app::{BdfApp, SidebarTab};
use crate::theme::{self, icon};
use crate::view::Jump;
use bdf_core::engine::{DocId, OutlineItem, TileKey, TileKind, tile_grid};
use bdf_core::geom::{page_px, quantize_scale};
use eframe::egui::{
    self, Color32, Context, CornerRadius, Id, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

enum Nav {
    Page(usize),
    Jump(Jump),
}

impl BdfApp {
    pub fn sidebar(&mut self, ui: &mut Ui, ctx: &Context) {
        let width = self.settings.sidebar_width;
        let r = egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(width)
            .size_range(150.0..=520.0)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::same(6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (t, glyph, tip) in [
                        (SidebarTab::Thumbs, icon::THUMBS, "Pages"),
                        (SidebarTab::Outline, icon::OUTLINE, "Outline"),
                        (SidebarTab::Bookmarks, icon::BOOKMARKS, "Bookmarks"),
                        (SidebarTab::Annots, icon::COMMENT, "Annotations"),
                        (SidebarTab::Search, icon::SEARCH, "Search results"),
                    ] {
                        let sel = self.sidebar_tab == t;
                        if ui
                            .selectable_label(sel, RichText::new(glyph).size(17.0))
                            .on_hover_text(tip)
                            .clicked()
                        {
                            self.sidebar_tab = t;
                        }
                    }
                });
                ui.separator();
                let Some(tab) = self.panes[self.active_pane].current() else {
                    ui.weak("No document");
                    return;
                };
                let doc = tab.doc;
                let current = tab.view.current_page;
                let nav = match self.sidebar_tab {
                    SidebarTab::Thumbs => self.thumbs(ui, ctx, doc, current),
                    SidebarTab::Outline => self.outline(ui, doc),
                    SidebarTab::Bookmarks => self.bookmarks(ui, ctx, doc, current),
                    SidebarTab::Annots => self.annot_list(ui, doc),
                    SidebarTab::Search => self.search_list(ui, doc),
                };
                if let Some(n) = nav
                    && let Some(t) = self.panes[self.active_pane].current_mut()
                {
                    match n {
                        Nav::Page(p) => t.view.goto(p, true),
                        Nav::Jump(j) => t.view.jump(j, true),
                    }
                }
            });
        let w = r.response.rect.width();
        if (w - self.settings.sidebar_width).abs() > 1.0 {
            self.settings.sidebar_width = w;
            self.settings_changed = true;
        }
    }

    fn thumbs(&mut self, ui: &mut Ui, _ctx: &Context, doc: DocId, current: usize) -> Option<Nav> {
        let d = self.docs.get(&doc)?;
        let n = d.page_count();
        let rotation = self.panes[self.active_pane]
            .current()
            .map(|t| t.view.rotation)
            .unwrap_or_default();
        let thumb_w = (ui.available_width() - 24.0).clamp(60.0, 180.0);
        let row_h = thumb_w * 1.3 + 30.0;
        let pal = theme::palette(ui.visuals().dark_mode);
        let mut nav = None;
        let follow = self.thumbs_scrolled_to != Some((doc, current));
        let mut area = egui::ScrollArea::vertical()
            .id_salt(("thumbs", doc.0))
            .auto_shrink([false, false]);
        if follow {
            let avail = ui.available_height();
            // show_rows adds the item spacing between rows.
            let pitch = row_h + ui.spacing().item_spacing.y;
            area = area.vertical_scroll_offset(
                (current as f32 * pitch - avail * 0.5 + row_h * 0.5).max(0.0),
            );
            self.thumbs_scrolled_to = Some((doc, current));
        }
        let ids: Vec<(usize, (f32, f32), u32)> = (0..n)
            .map(|p| (p, d.info.page_sizes[p], d.generation(p as u32)))
            .collect();
        let labels: Vec<Option<String>> = d.info.page_labels.clone();
        let dark_pages = self.settings.dark_pages;
        let tiles = &mut self.tiles;
        let ppp = ui.ctx().pixels_per_point();
        area.show_rows(ui, row_h, n, |ui, range| {
            for p in range {
                let (page, size, generation) = ids[p];
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                // Rendered for the exact device pixels it covers, and placed on
                // the pixel grid, so thumbnails are sharp at any display scaling.
                let size_rot = rotation.apply_size(size);
                let fit = (thumb_w / size_rot.0).min((row_h - 30.0) / size_rot.1);
                let q = quantize_scale(fit * ppp);
                let (pw, ph) = page_px(size_rot, q);
                let centre = pos2(rect.center().x, rect.min.y + 6.0 + (row_h - 30.0) * 0.5);
                let min = (centre.to_vec2() * ppp - vec2(pw as f32, ph as f32) * 0.5).round() / ppp;
                let img = egui::Rect::from_min_size(
                    min.to_pos2(),
                    vec2(pw as f32 / ppp, ph as f32 / ppp),
                );
                let painter = ui.painter();
                if page == current {
                    painter.rect_filled(rect.shrink(2.0), CornerRadius::same(8), pal.accent_soft);
                } else if resp.hovered() {
                    painter.rect_filled(
                        rect.shrink(2.0),
                        CornerRadius::same(8),
                        ui.visuals().widgets.hovered.weak_bg_fill,
                    );
                }
                painter.add(
                    egui::epaint::Shadow {
                        offset: [0, 1],
                        blur: 6,
                        spread: 0,
                        color: pal.page_shadow,
                    }
                    .as_shape(img, CornerRadius::same(2)),
                );
                painter.rect_filled(
                    img,
                    CornerRadius::same(1),
                    if dark_pages {
                        Color32::from_gray(18)
                    } else {
                        Color32::WHITE
                    },
                );
                let thumb = TileKey {
                    doc,
                    page: page as u32,
                    rotation,
                    invert: dark_pages,
                    generation,
                    kind: TileKind::Thumb,
                };
                let priority = 3000 + (page as i64 - current as i64).unsigned_abs() as u32;
                // One tile holds the whole page at thumbnail size; the shared
                // low-resolution thumbnail stands in while it renders.
                let image = if tile_grid(size_rot, q) == (1, 1) {
                    let sharp = TileKey {
                        kind: TileKind::Tile {
                            scale: q,
                            tx: 0,
                            ty: 0,
                        },
                        ..thumb
                    };
                    tiles.want(sharp, priority).or_else(|| tiles.cached(&thumb))
                } else {
                    tiles.want(thumb, priority)
                };
                if let Some(t) = image {
                    painter.image(
                        t.id(),
                        img,
                        egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                if page == current {
                    painter.rect_stroke(
                        img.expand(2.0),
                        CornerRadius::same(3),
                        Stroke::new(2.0, pal.accent),
                        StrokeKind::Outside,
                    );
                }
                let label = labels
                    .get(page)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| (page + 1).to_string());
                painter.text(
                    pos2(rect.center().x, rect.max.y - 12.0),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
                if resp.clicked() {
                    nav = Some(Nav::Page(page));
                }
            }
        });
        if nav.is_some() {
            // Do not re-centre the list on a click inside it.
            if let Some(Nav::Page(p)) = &nav {
                self.thumbs_scrolled_to = Some((doc, *p));
            }
        }
        nav
    }

    fn outline(&mut self, ui: &mut Ui, doc: DocId) -> Option<Nav> {
        let d = self.docs.get(&doc)?;
        if d.info.outline.is_empty() {
            ui.weak("This document has no outline.");
            return None;
        }
        let outline = d.info.outline.clone();
        let mut nav = None;
        egui::ScrollArea::vertical()
            .id_salt(("outline", doc.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, item) in outline.iter().enumerate() {
                    outline_item(
                        ui,
                        item,
                        Id::unique(("ol", doc.0, i)),
                        0,
                        &mut nav,
                        &mut self.outline_uri,
                    );
                }
            });
        if let Some(u) = self.outline_uri.take() {
            self.show_dialog(crate::app::Dialog::OpenUri(u));
        }
        nav
    }

    fn bookmarks(&mut self, ui: &mut Ui, ctx: &Context, doc: DocId, current: usize) -> Option<Nav> {
        let fp = self.docs.get(&doc)?.info.fingerprint.clone();
        if ui
            .button(format!("{}  Bookmark this page", icon::PLUS))
            .clicked()
        {
            self.add_bookmark(ctx, doc, current as u32);
        }
        ui.add_space(4.0);
        let mut nav = None;
        let mut remove = None;
        let mut changed = false;
        let list = self.settings.doc_mut(&fp);
        if list.bookmarks.is_empty() {
            ui.weak("No bookmarks yet. Press Ctrl+B to bookmark the current page.");
        }
        egui::ScrollArea::vertical()
            .id_salt(("bm", doc.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, b) in list.bookmarks.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let id = Id::unique(("bm-edit", doc.0, i));
                        let editing = ui.data(|m| m.get_temp::<bool>(id)).unwrap_or(false);
                        if editing {
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut b.title)
                                    .desired_width(ui.available_width() - 40.0),
                            );
                            r.request_focus();
                            if r.lost_focus() {
                                ui.data_mut(|m| m.insert_temp(id, false));
                                changed = true;
                            }
                        } else {
                            let r = ui.add(
                                egui::Button::new(format!("{}  {}", icon::BOOKMARK, b.title))
                                    .right_text(RichText::new((b.page + 1).to_string()).weak())
                                    .min_size(vec2(ui.available_width() - 30.0, 24.0)),
                            );
                            if r.clicked() {
                                nav = Some(Nav::Page(b.page as usize));
                            }
                            if r.double_clicked() {
                                ui.data_mut(|m| m.insert_temp(id, true));
                            }
                            r.on_hover_text("Click to go there, double-click to rename");
                        }
                        if ui
                            .small_button(icon::TRASH)
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                }
            });
        if let Some(i) = remove {
            self.settings.doc_mut(&fp).bookmarks.remove(i);
            changed = true;
        }
        if changed {
            self.settings_changed = true;
        }
        nav
    }

    fn annot_list(&mut self, ui: &mut Ui, doc: DocId) -> Option<Nav> {
        // Scan on first view, and again after edits once the last scan finished.
        let rescan = self
            .docs
            .get(&doc)
            .is_some_and(|d| d.annots.ticket == 0 || (d.annots.stale && d.annots.complete));
        if rescan {
            self.scan_annots(doc);
        }
        let d = self.docs.get(&doc)?;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} annotation(s)", d.annots.items.len())).strong());
            if !d.annots.complete {
                crate::theme::spinner(ui, 14.0);
                ui.weak(format!("{} / {}", d.annots.pages_done, d.page_count()));
            }
        });
        let mut rescan = false;
        if d.annots.complete && ui.small_button("Refresh").clicked() {
            rescan = true;
        }
        let items = d.annots.items.clone();
        let mut nav = None;
        egui::ScrollArea::vertical()
            .id_salt(("annots", doc.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut last_page = None;
                for (page, a) in &items {
                    if last_page != Some(*page) {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("Page {}", page + 1))
                                .weak()
                                .size(12.0),
                        );
                        last_page = Some(*page);
                    }
                    let color = a
                        .color
                        .map(|c| Color32::from_rgb(c[0], c[1], c[2]))
                        .unwrap_or(Color32::GRAY);
                    let text = if a.contents.is_empty() {
                        a.kind.label().to_string()
                    } else {
                        format!("{}: {}", a.kind.label(), a.contents)
                    };
                    let r = ui.add(
                        egui::Button::new(
                            RichText::new(format!("●  {}", one_line(&text, 60)))
                                .color(ui.visuals().text_color()),
                        )
                        .min_size(vec2(ui.available_width(), 24.0)),
                    );
                    ui.painter()
                        .circle_filled(r.rect.left_center() + vec2(12.0, 0.0), 4.0, color);
                    let r = if a.author.is_empty() {
                        r
                    } else {
                        r.on_hover_text(format!("{}\n{}", a.author, a.contents))
                    };
                    if r.clicked() {
                        nav = Some(Nav::Jump(Jump {
                            page: *page as usize,
                            y_user: None,
                            rect_user: Some(a.rect),
                        }));
                    }
                }
            });
        if rescan {
            self.scan_annots(doc);
        }
        nav
    }

    fn search_list(&mut self, ui: &mut Ui, doc: DocId) -> Option<Nav> {
        const ROW: f32 = 46.0;
        const BADGE_W: f32 = 38.0;
        let pal = theme::palette(ui.visuals().dark_mode);
        let d = self.docs.get_mut(&doc)?;
        let Some(s) = d.search.as_mut() else {
            ui.add_space(8.0);
            ui.weak("Press Ctrl+F to search this document.");
            return None;
        };

        // Summary: how many, for what, where.
        let pages = s.hits.windows(2).filter(|w| w[0].page != w[1].page).count()
            + usize::from(!s.hits.is_empty());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let n = s.count_label();
            let text = match (s.hits.len(), s.done) {
                (0, true) => "No matches".to_string(),
                (1, _) => "1 match".to_string(),
                _ => format!("{n} matches"),
            };
            ui.label(RichText::new(text).size(15.0).strong());
            if !s.done {
                crate::theme::spinner(ui, 14.0);
            }
        });
        let detail = if s.done {
            match pages {
                0 => format!("for “{}”", s.query),
                1 => format!("for “{}” on 1 page", s.query),
                p => format!("for “{}” on {p} pages", s.query),
            }
        } else {
            format!(
                "for “{}” · searching {} of {} pages",
                s.query, s.searched, s.total
            )
        };
        ui.label(RichText::new(detail).size(12.0).color(pal.muted));
        if s.hits.is_empty() && s.done && (s.opts.match_case || s.opts.whole_word) {
            ui.label(
                RichText::new("Match case or Whole words is on; try turning it off.")
                    .size(12.0)
                    .color(pal.muted),
            );
        }
        ui.add_space(6.0);
        ui.separator();

        // Follow the current match (F3, Enter) when it is out of sight.
        let follow_id = Id::unique(("search-follow", doc.0));
        let (seen, visible) = ui
            .data(|m| m.get_temp::<(Option<usize>, std::ops::Range<usize>)>(follow_id))
            .unwrap_or((None, 0..0));
        let mut area = egui::ScrollArea::vertical()
            .id_salt(("search", doc.0))
            .auto_shrink([false, false]);
        if let Some(c) = s.current
            && seen != s.current
            && !(visible.start < c && c + 1 < visible.end)
        {
            area = area.vertical_scroll_offset((c as f32 * ROW - ROW * 2.0).max(0.0));
        }
        let mut pick = None;
        let mut shown = 0..0;
        area.show_rows(ui, ROW, s.hits.len(), |ui, range| {
            shown = range.clone();
            let text_color = ui.visuals().strong_text_color();
            let context_color = ui.visuals().text_color();
            for i in range {
                let h = &s.hits[i];
                let sel = s.current == Some(i);
                let first_on_page = i == 0 || s.hits[i - 1].page != h.page;
                let (rect, r) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                let p = ui.painter();
                if first_on_page && i > 0 {
                    p.hline(
                        rect.x_range(),
                        rect.top(),
                        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    );
                }
                let body = rect.shrink2(vec2(0.0, 2.0));
                if sel {
                    p.rect_filled(body, CornerRadius::same(6), pal.accent_soft);
                    p.rect_filled(
                        egui::Rect::from_min_size(body.min, vec2(3.0, body.height())),
                        CornerRadius::same(2),
                        pal.accent,
                    );
                } else if r.hovered() {
                    p.rect_filled(
                        body,
                        CornerRadius::same(6),
                        ui.visuals().widgets.hovered.weak_bg_fill,
                    );
                }
                if first_on_page {
                    let badge = egui::Rect::from_min_size(
                        body.min + vec2(7.0, 5.0),
                        vec2(BADGE_W - 8.0, 18.0),
                    );
                    let (fill, ink) = if sel {
                        (pal.accent, Color32::WHITE)
                    } else {
                        (ui.visuals().widgets.inactive.bg_fill, text_color)
                    };
                    p.rect_filled(badge, CornerRadius::same(5), fill);
                    p.text(
                        badge.center(),
                        egui::Align2::CENTER_CENTER,
                        (h.page + 1).to_string(),
                        egui::FontId::proportional(11.5),
                        ink,
                    );
                }
                let job = snippet_job(h, pal.search_hit, text_color, context_color);
                let text_rect = egui::Rect::from_min_max(
                    pos2(body.left() + BADGE_W + 6.0, body.top() + 4.0),
                    pos2(body.right() - 6.0, body.bottom()),
                );
                let mut job = job;
                job.wrap.max_width = text_rect.width();
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                p.galley(text_rect.min, galley, text_color);
                let r = r.on_hover_text(format!("Page {}", h.page + 1));
                if r.clicked() {
                    pick = Some(i);
                }
            }
        });
        ui.data_mut(|m| m.insert_temp(follow_id, (s.current, shown)));
        let mut nav = None;
        if let Some(i) = pick {
            s.current = Some(i);
            let h = &s.hits[i];
            nav = Some(Nav::Jump(Jump {
                page: h.page as usize,
                y_user: None,
                rect_user: h.rects.first().copied(),
            }));
        }
        nav
    }
}

/// Two lines of excerpt with the match highlighted, starting shortly before
/// the match so it is always in view.
fn snippet_job(
    h: &bdf_core::worker::SearchHit,
    mark: Color32,
    strong: Color32,
    context: Color32,
) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    const LEAD: usize = 22;
    let font = egui::FontId::proportional(12.5);
    let (s, m) = (&h.snippet, h.snippet_match.clone());
    let m = if s.is_char_boundary(m.start) && s.is_char_boundary(m.end) && m.end <= s.len() {
        m
    } else {
        0..0
    };
    let before = &s[..m.start];
    let lead: String = if before.chars().count() > LEAD {
        let tail: String = before
            .chars()
            .rev()
            .take(LEAD)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // Start at a word, not in the middle of one.
        let tail = tail
            .split_once(' ')
            .map_or(tail.as_str(), |(_, t)| t)
            .to_string();
        format!("…{tail}")
    } else {
        before.to_string()
    };
    let plain = TextFormat::simple(font.clone(), context);
    let mut job = LayoutJob::default();
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    job.append(&lead, 0.0, plain.clone());
    job.append(
        &s[m.clone()],
        0.0,
        TextFormat {
            color: strong,
            background: mark,
            ..TextFormat::simple(font, strong)
        },
    );
    job.append(&s[m.end..], 0.0, plain);
    job
}

fn one_line(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s
    }
}

fn outline_item(
    ui: &mut Ui,
    item: &OutlineItem,
    id: Id,
    depth: usize,
    nav: &mut Option<Nav>,
    uri: &mut Option<String>,
) {
    let title = if item.title.is_empty() {
        "(untitled)"
    } else {
        item.title.as_str()
    };
    let mut click = |ui: &mut Ui| {
        let r = ui.add(
            egui::Button::new(RichText::new(title))
                .min_size(vec2(ui.available_width(), 22.0))
                .truncate(),
        );
        if r.clicked() {
            if let Some(p) = item.page {
                *nav = Some(Nav::Jump(Jump {
                    page: p as usize,
                    y_user: item.y,
                    rect_user: None,
                }));
            } else if let Some(u) = &item.uri {
                *uri = Some(u.clone());
            }
        }
        r.on_hover_text(match item.page {
            Some(p) => format!("{title}\npage {}", p + 1),
            None => title.to_string(),
        });
    };
    if item.children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            click(ui);
        });
    } else {
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, depth == 0)
            .show_header(ui, |ui| click(ui))
            .body(|ui| {
                for (i, c) in item.children.iter().enumerate() {
                    outline_item(ui, c, id.with(i), depth + 1, nav, uri);
                }
            });
    }
}
