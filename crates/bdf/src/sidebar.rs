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
        let d = self.docs.get_mut(&doc)?;
        let Some(s) = d.search.as_mut() else {
            ui.weak("Press Ctrl+F to search this document.");
            return None;
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} result(s) for “{}”", s.count_label(), s.query)).strong(),
            );
            if !s.done {
                crate::theme::spinner(ui, 14.0);
            }
        });
        let mut nav = None;
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt(("search", doc.0))
            .auto_shrink([false, false])
            .show_rows(ui, 40.0, s.hits.len(), |ui, range| {
                for i in range {
                    let h = &s.hits[i];
                    let sel = s.current == Some(i);
                    let r = ui.add(
                        egui::Button::selectable(
                            sel,
                            RichText::new(format!(
                                "{}\n{}",
                                one_line(&h.snippet, 70),
                                RichText::new(format!("page {}", h.page + 1)).text()
                            ))
                            .size(12.5),
                        )
                        .min_size(vec2(ui.available_width(), 36.0)),
                    );
                    if r.clicked() {
                        pick = Some(i);
                    }
                }
            });
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
