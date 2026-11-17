//! Offline Survival Guides: search installed packs, marketplace, article reader.

use dioxus::prelude::*;

use crate::guides::{
    self, CatalogPack, JobStatus, LoadedArticle, SearchHit, GUIDE_CATEGORIES,
};

use super::icons::{Icon, IconName};
use super::widgets::{
    Button, ButtonKind, PageHeading, SelectInput, SlideDownPanel, TextInput, Throbber,
};

/// Scroll the active find mark into view inside the slide-down body (or page).
fn scroll_guides_find_match(index: usize) {
    let _ = document::eval(&format!(
        r#"(function() {{
  var marks = document.querySelectorAll('mark.guides-find-hit');
  if (!marks.length) return;
  marks.forEach(function(m) {{
    m.classList.remove('guides-find-current');
    m.removeAttribute('id');
  }});
  var i = {index};
  if (i < 0 || i >= marks.length) i = 0;
  var el = marks[i];
  el.classList.add('guides-find-current');
  el.id = 'gfind-current';
  var scroller = el.closest('.slide-down-body');
  if (scroller) {{
    var chrome = scroller.querySelector('.guides-reader-chrome');
    var pad = chrome ? (chrome.getBoundingClientRect().height + 12) : 56;
    var top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    scroller.scrollTop = Math.max(0, top - pad);
  }} else {{
    el.scrollIntoView({{ behavior: 'instant', block: 'center' }});
  }}
}})()"#
    ));
}

/// Bind in-article `#Page_N` / `#Chapter_…` (and `[data-guides-jump]`) to scroll the slide-down body.
/// Must `.await` the eval future or the script may never run.
fn bind_guides_internal_jumps() {
    spawn(async move {
        // Wait a tick so dangerous_inner_html is in the DOM before we bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // `document::eval` takes the script as-is (no Rust format!). Use single `{`.
        // r## so a JS `"#…"` cannot end the string early.
        let result = document::eval(
            r##"(function() {
  if (window.__guidesJumpBound) return 'already';
  window.__guidesJumpBound = true;
  function jumpToId(id) {
    if (!id) return;
    try { id = decodeURIComponent(id); } catch (_) {}
    var el = document.getElementById(id);
    if (!el) return;
    var scroller = el.closest('.slide-down-body');
    if (scroller) {
      var chrome = scroller.querySelector('.guides-reader-chrome');
      var pad = chrome ? (chrome.getBoundingClientRect().height + 12) : 56;
      var top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
      scroller.scrollTop = Math.max(0, top - pad);
    } else {
      el.scrollIntoView({ behavior: 'auto', block: 'start' });
    }
  }
  window.__guidesJumpTo = jumpToId;
  function jumpIdFromEl(el) {
    if (!el) return '';
    return el.getAttribute('data-guides-jump') || '';
  }
  document.addEventListener('click', function(e) {
    var t = e.target;
    if (!t || !t.closest) return;
    // Block ANY leftover <a> inside the reader — wry opens http(s) in the OS browser.
    var anyA = t.closest('.guides-article a');
    if (anyA) {
      e.preventDefault();
      e.stopPropagation();
    }
    // Jump controls are <button class="guides-jump" data-guides-jump="…">.
    var jumpEl = t.closest('.guides-article .guides-jump, .guides-article [data-guides-jump]');
    var id = jumpIdFromEl(jumpEl);
    if (!id) {
      var tr = t.closest('.guides-article tr');
      if (tr) {
        var link = tr.querySelector('.guides-jump, [data-guides-jump]');
        if (!link && tr.previousElementSibling) {
          link = tr.previousElementSibling.querySelector('.guides-jump, [data-guides-jump]');
        }
        id = jumpIdFromEl(link);
      }
    }
    if (!id) return;
    e.preventDefault();
    e.stopPropagation();
    jumpToId(id);
  }, true);
  return 'bound';
})()"##,
        )
        .await;
        if let Err(err) = result {
            hq_log!(warn, "guides jump bind failed: {err}");
        }
    });
}

fn status_note(message: impl Into<String>) {
    let message = message.into();
    let kind = crate::toast::infer_kind(&message);
    crate::toast::push(kind, message);
}

/// Load an article off the UI thread so Gutenberg-scale HTML cannot freeze GTK/WebKit.
fn spawn_load_article(
    mut reader: Signal<Option<LoadedArticle>>,
    mut loading: Signal<bool>,
    mut load_gen: Signal<u64>,
    pack_id: String,
    article_id: String,
) {
    let gen = load_gen() + 1;
    load_gen.set(gen);
    loading.set(true);
    spawn(async move {
        let pack = pack_id.clone();
        let art = article_id.clone();
        let result =
            tokio::task::spawn_blocking(move || guides::load_article(&pack, &art)).await;
        if load_gen() != gen {
            return;
        }
        loading.set(false);
        match result {
            Ok(Ok(a)) => reader.set(Some(a)),
            Ok(Err(err)) => status_note(err),
            Err(err) => status_note(format!("Could not load article ({err}).")),
        }
    });
}

/// Visible search hits before "Load more".
const GUIDES_LIST_PAGE: usize = 25;

fn status_class(status: &str) -> &'static str {
    match status {
        "installed" => "guides-status guides-status-installed",
        "available" => "guides-status guides-status-available",
        _ => "guides-status guides-status-unavailable",
    }
}

#[component]
pub fn GuidesPage() -> Element {
    let mut tick = use_signal(|| 0u64);
    let mut query = use_signal(String::new);
    let mut pack_filter = use_signal(|| None::<String>);
    let mut category_filter = use_signal(|| None::<String>);
    let mut list_limit = use_signal(|| GUIDES_LIST_PAGE);
    let mut market_open = use_signal(|| false);
    let mut reader: Signal<Option<LoadedArticle>> = use_signal(|| None);
    let mut reader_loading = use_signal(|| false);
    let mut reader_load_gen = use_signal(|| 0u64);
    let mut catalog_tick = use_signal(|| 0u64);
    let mut poll_started = use_signal(|| false);
    let mut prev_job_running = use_signal(|| false);

    if !poll_started() {
        poll_started.set(true);
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                tick.set(tick() + 1);
            }
        });
    }

    let _ = catalog_tick();
    let job = guides::job_status();
    // Only subscribe to the poll tick while Marketplace is open or a job is running —
    // otherwise the reader re-renders a full Gutenberg page every 500ms and scrolling stalls.
    if market_open() || job.running {
        let _ = tick();
    }
    // Toast once when an install/sync job finishes.
    if prev_job_running() != job.running {
        if prev_job_running() && !job.running {
            if job.phase == "done" {
                let msg = if job.detail.is_empty() {
                    "Guides job finished.".into()
                } else {
                    job.detail.clone()
                };
                crate::toast::push(crate::toast::ToastKind::Success, msg);
            } else if !job.error.is_empty() || job.phase == "error" {
                let msg = if job.error.is_empty() {
                    "Guides job failed.".into()
                } else {
                    job.error.clone()
                };
                crate::toast::push(crate::toast::ToastKind::Error, msg);
            }
            catalog_tick.set(catalog_tick() + 1);
        }
        prev_job_running.set(job.running);
    }

    let catalog = guides::load_catalog();
    let installed: Vec<CatalogPack> = guides::list_installed_packs();

    let filter = pack_filter();
    let cat_filter = category_filter();
    let hits: Vec<SearchHit> =
        match guides::search(&query(), filter.as_deref(), cat_filter.as_deref()) {
            Ok(h) => h,
            Err(err) => {
                hq_log!(warn, "guides search: {err}");
                Vec::new()
            }
        };
    let hits_total = hits.len();
    let visible_limit = list_limit().min(hits_total);
    let visible_hits: Vec<SearchHit> = hits.into_iter().take(visible_limit).collect();
    let pack_select_value = pack_filter().unwrap_or_default();
    let category_select_value = category_filter().unwrap_or_default();

    rsx! {
        div { class: "flex flex-col gap-4",
            PageHeading {
                icon: IconName::Book,
                title: "Guides".to_string(),
                "Offline survival and reference packs. Download while online, search and read with no internet. Community reference only — not medical, legal, or emergency-authority advice."
            }

            div { class: "guides-toolbar",
                div { class: "guides-toolbar__search",
                    label { class: "mb-1 block text-xs font-medium text-muted-foreground", "Search" }
                    TextInput {
                        value: query(),
                        placeholder: "Search installed guides…".to_string(),
                        oninput: move |evt: FormEvent| {
                            query.set(evt.value());
                            list_limit.set(GUIDES_LIST_PAGE);
                        },
                    }
                }
                div { class: "guides-toolbar__select",
                    label {
                        class: "mb-1 block text-xs font-medium text-muted-foreground",
                        r#for: "guides-category-filter",
                        "Category"
                    }
                    SelectInput {
                        id: "guides-category-filter".to_string(),
                        value: category_select_value.clone(),
                        onchange: move |evt: FormEvent| {
                            let v = evt.value();
                            if v.trim().is_empty() {
                                category_filter.set(None);
                            } else {
                                category_filter.set(Some(v));
                            }
                            list_limit.set(GUIDES_LIST_PAGE);
                        },
                        option { value: "", "All categories" }
                        for cat in GUIDE_CATEGORIES.iter() {
                            option { value: "{cat.id}", "{cat.label}" }
                        }
                    }
                }
                if !installed.is_empty() {
                    div { class: "guides-toolbar__select",
                        label {
                            class: "mb-1 block text-xs font-medium text-muted-foreground",
                            r#for: "guides-pack-filter",
                            "Pack"
                        }
                        SelectInput {
                            id: "guides-pack-filter".to_string(),
                            value: pack_select_value.clone(),
                            onchange: move |evt: FormEvent| {
                                let v = evt.value();
                                if v.trim().is_empty() {
                                    pack_filter.set(None);
                                } else {
                                    pack_filter.set(Some(v));
                                }
                                list_limit.set(GUIDES_LIST_PAGE);
                            },
                            option { value: "", "All installed packs" }
                            for pack in installed.iter() {
                                option { value: "{pack.id}", "{pack.name}" }
                            }
                        }
                    }
                }
                div { class: "guides-toolbar__actions",
                    Button {
                        kind: ButtonKind::Primary,
                        onclick: move |_| market_open.set(true),
                        Icon { name: IconName::Folder, size: 14 }
                        "Marketplace"
                    }
                    if !installed.is_empty()
                        || category_filter().is_some()
                        || !query().trim().is_empty()
                    {
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                pack_filter.set(None);
                                category_filter.set(None);
                                query.set(String::new());
                                list_limit.set(GUIDES_LIST_PAGE);
                            },
                            "Clear filters"
                        }
                    }
                }
            }

            if installed.is_empty() {
                section { class: "guides-empty",
                    div { class: "mb-3 flex justify-center text-sky-300",
                        Icon { name: IconName::Book, size: 28 }
                    }
                    p { class: "text-sm text-muted-foreground",
                        "No guide packs installed yet. Open Marketplace to install the bundled sample or sync the catalog when online."
                    }
                    div { class: "mt-4 flex justify-center",
                        Button {
                            kind: ButtonKind::Primary,
                            onclick: move |_| market_open.set(true),
                            "Open Marketplace"
                        }
                    }
                }
            } else if hits_total == 0 {
                p { class: "text-sm text-muted-foreground",
                    if !query().trim().is_empty() {
                        "No matches for that search."
                    } else if category_filter().is_some() || pack_filter().is_some() {
                        "No articles match the selected filters."
                    } else {
                        "No articles in installed packs."
                    }
                }
            } else {
                div { class: "guides-results",
                    for (row_i, hit) in visible_hits.into_iter().enumerate() {
                        {
                            let pack_id = hit.pack_id.clone();
                            let article_id = hit.article_id.clone();
                            let title = hit.title.clone();
                            let pack_name = hit.pack_name.clone();
                            let snippet = hit.snippet.clone();
                            let tags = hit.tags.clone();
                            let row_class = if row_i % 2 == 1 {
                                "guides-result row-zebra"
                            } else {
                                "guides-result"
                            };
                            rsx! {
                                button {
                                    class: "{row_class}",
                                    r#type: "button",
                                    onclick: move |_| {
                                        spawn_load_article(
                                            reader,
                                            reader_loading,
                                            reader_load_gen,
                                            pack_id.clone(),
                                            article_id.clone(),
                                        );
                                    },
                                    span { class: "guides-result__icon",
                                        Icon { name: IconName::Book, size: 16 }
                                    }
                                    div { class: "guides-result__body",
                                        div { class: "guides-result__title-row",
                                            span { class: "guides-result__title", "{title}" }
                                            span { class: "guides-result__pack", "{pack_name}" }
                                        }
                                        if !snippet.is_empty() {
                                            p { class: "guides-result__snippet", "{snippet}" }
                                        }
                                        if !tags.is_empty() {
                                            div { class: "guides-tag-row",
                                                for tag in tags.iter() {
                                                    span { class: "guides-tag", "{tag}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if list_limit() < hits_total {
                        div { class: "guides-more",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    list_limit.set(
                                        list_limit()
                                            .saturating_add(GUIDES_LIST_PAGE)
                                            .min(hits_total),
                                    );
                                },
                                "Load more · {hits_total.saturating_sub(list_limit())} remaining"
                            }
                        }
                    }
                }
            }
        }

        MarketplacePanel {
            open: market_open(),
            catalog: catalog.clone(),
            job: job.clone(),
            on_close: move |_| market_open.set(false),
            on_changed: move |_| {
                catalog_tick.set(catalog_tick() + 1);
                tick.set(tick() + 1);
            },
        }

        ArticleReader {
            article: reader(),
            loading: reader_loading(),
            on_close: move |_| {
                reader_load_gen.set(reader_load_gen() + 1);
                reader_loading.set(false);
                reader.set(None);
            },
            on_navigate: move |(pack, art): (String, String)| {
                spawn_load_article(reader, reader_loading, reader_load_gen, pack, art);
            },
        }
    }
}

#[component]
fn MarketplacePanel(
    open: bool,
    catalog: guides::GuidesCatalog,
    job: JobStatus,
    on_close: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let mut market_category = use_signal(|| None::<String>);
    let market_cat_value = market_category().unwrap_or_default();
    let visible_packs: Vec<&CatalogPack> = catalog
        .packs
        .iter()
        .filter(|p| p.matches_category(market_category().as_deref()))
        .collect();

    let footer = rsx! {
        div { class: "flex flex-wrap justify-end gap-2",
            Button {
                kind: ButtonKind::Ghost,
                disabled: job.running,
                onclick: move |_| {
                    guides::spawn_sync_catalog();
                    on_changed.call(());
                    status_note("Refreshing guide catalog…");
                },
                "Sync catalog"
            }
            Button {
                kind: ButtonKind::Ghost,
                onclick: move |_| on_close.call(()),
                "Close"
            }
        }
    };

    rsx! {
        SlideDownPanel {
            open,
            title: "Guide Marketplace".to_string(),
            subtitle: "Install packs while online. Content stays under this computer’s data folder, separate from metis.db.".to_string(),
            icon: Some(IconName::Folder),
            wide: true,
            on_close: move |_| on_close.call(()),
            footer: Some(footer),
            if job.running {
                div { class: "guides-progress mb-3",
                    div { class: "guides-progress__head",
                        Throbber { size: 14 }
                        span { class: "guides-progress__detail", "{job.detail}" }
                        if job.bytes_total > 0 {
                            span { class: "guides-progress__pct", "{job.percent}%" }
                        }
                    }
                    div {
                        class: if job.bytes_total > 0 {
                            "guides-progress__track"
                        } else {
                            "guides-progress__track guides-progress__track--indeterminate"
                        },
                        div {
                            class: "guides-progress__bar",
                            style: if job.bytes_total > 0 {
                                format!("width: {}%", job.percent)
                            } else {
                                String::new()
                            },
                        }
                    }
                }
            }
            div { class: "guides-market-filter mb-3",
                label {
                    class: "mb-1 block text-xs font-medium text-muted-foreground",
                    r#for: "guides-market-category",
                    "Category"
                }
                SelectInput {
                    id: "guides-market-category".to_string(),
                    value: market_cat_value.clone(),
                    onchange: move |evt: FormEvent| {
                        let v = evt.value();
                        if v.trim().is_empty() {
                            market_category.set(None);
                        } else {
                            market_category.set(Some(v));
                        }
                    },
                    option { value: "", "All categories" }
                    for cat in GUIDE_CATEGORIES.iter() {
                        option { value: "{cat.id}", "{cat.label}" }
                    }
                }
            }
            if catalog.packs.is_empty() {
                p { class: "text-sm text-muted-foreground", "Catalog is empty." }
            } else if visible_packs.is_empty() {
                p { class: "text-sm text-muted-foreground", "No packs in this category." }
            } else {
                div { class: "guides-market",
                    for (row_i, pack) in visible_packs.into_iter().enumerate() {
                        {
                            let id = pack.id.clone();
                            let id_rm = pack.id.clone();
                            let can_install = pack.is_installable();
                            let installed = pack.status == "installed";
                            let name = pack.name.clone();
                            let desc = pack.description.clone();
                            let source = pack.source.clone();
                            let version = pack.version.clone();
                            let license = pack.license.clone();
                            let status = pack.status.clone();
                            let format = pack.format.clone();
                            let cat_labels = pack.category_labels();
                            let status_cls = status_class(&status);
                            let row_class = if row_i % 2 == 1 {
                                "guides-market-row row-zebra"
                            } else {
                                "guides-market-row"
                            };
                            rsx! {
                                div { class: "{row_class}",
                                    div { class: "guides-market-main",
                                        span { class: "guides-result__icon",
                                            Icon { name: IconName::Book, size: 16 }
                                        }
                                        div { class: "guides-result__body",
                                            div { class: "guides-result__title-row",
                                                span { class: "guides-result__title", "{name}" }
                                                span { class: "{status_cls}",
                                                    if installed {
                                                        Icon { name: IconName::Check, size: 12 }
                                                    }
                                                    "{status}"
                                                }
                                            }
                                            p { class: "guides-result__snippet", "{desc}" }
                                            if !cat_labels.is_empty() {
                                                div { class: "guides-tag-row",
                                                    for label in cat_labels.iter() {
                                                        span { class: "guides-cat-chip", "{label}" }
                                                    }
                                                }
                                            }
                                            div { class: "guides-market-meta",
                                                span { class: "guides-market-source", "{source}" }
                                                span { "v{version}" }
                                                span { "{license}" }
                                                span { "{format}" }
                                            }
                                        }
                                    }
                                    div { class: "guides-market-actions",
                                        if installed {
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                disabled: job.running,
                                                onclick: move |_| {
                                                    match guides::remove_pack(&id_rm) {
                                                        Ok(()) => {
                                                            status_note("Pack removed.");
                                                            on_changed.call(());
                                                        }
                                                        Err(err) => status_note(err),
                                                    }
                                                },
                                                Icon { name: IconName::Trash, size: 14 }
                                                "Remove"
                                            }
                                        }
                                        if can_install {
                                            {
                                                let name_toast = name.clone();
                                                rsx! {
                                                    Button {
                                                        kind: if installed {
                                                            ButtonKind::Ghost
                                                        } else {
                                                            ButtonKind::Primary
                                                        },
                                                        disabled: job.running,
                                                        onclick: move |_| {
                                                            let label = if installed {
                                                                format!("Reinstalling {name_toast}…")
                                                            } else {
                                                                format!("Installing {name_toast}…")
                                                            };
                                                            crate::toast::push(
                                                                crate::toast::ToastKind::Info,
                                                                label,
                                                            );
                                                            guides::spawn_install(id.clone());
                                                            on_changed.call(());
                                                        },
                                                        if installed {
                                                            "Reinstall"
                                                        } else {
                                                            "Install"
                                                        }
                                                    }
                                                }
                                            }
                                        } else if !installed {
                                            span { class: "guides-status guides-status-unavailable", "Soon" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            p { class: "mt-3 text-xs text-muted-foreground",
                "Disclaimer: third-party and community material. Metis Mesh HQ is not FEMA, NWS, or law enforcement. Packs never enroll devices or send mesh traffic."
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GuideFontSize {
    Small,
    Normal,
    Large,
}

impl GuideFontSize {
    fn article_class(self) -> &'static str {
        match self {
            GuideFontSize::Small => "guides-article guides-font-sm max-w-none leading-relaxed",
            GuideFontSize::Normal => "guides-article guides-font-md max-w-none leading-relaxed",
            GuideFontSize::Large => "guides-article guides-font-lg max-w-none leading-relaxed",
        }
    }
}

#[component]
fn ArticleReader(
    article: Option<LoadedArticle>,
    loading: bool,
    on_close: EventHandler<()>,
    on_navigate: EventHandler<(String, String)>,
) -> Element {
    let mut find_draft = use_signal(String::new);
    let mut find_applied = use_signal(String::new);
    let mut find_article_key = use_signal(String::new);
    let mut find_idx = use_signal(|| 0usize);
    let mut find_hits = use_signal(|| 0usize);
    let mut find_html = use_signal(|| None::<String>);
    let mut find_needle_stamp = use_signal(String::new);
    let mut find_scroll_token = use_signal(|| 0u64);
    let mut font_size = use_signal(|| GuideFontSize::Normal);
    let mut jumps_bound_for = use_signal(String::new);
    let open = article.is_some() || loading;
    if article.is_some() {
        let key = article
            .as_ref()
            .map(|a| format!("{}::{}", a.pack_id, a.article.id))
            .unwrap_or_default();
        // Bind once per opened article (not every parent re-render).
        if jumps_bound_for() != key {
            jumps_bound_for.set(key);
            // Reset then re-bind; await reset so a prior "already bound" flag cannot skip us.
            spawn(async move {
                let _ = document::eval("window.__guidesJumpBound = false;").await;
                bind_guides_internal_jumps();
            });
        }
    }
    let title = article
        .as_ref()
        .map(|a| a.article.title.clone())
        .unwrap_or_else(|| {
            if loading {
                "Loading…".into()
            } else {
                "Article".into()
            }
        });
    let subtitle = article
        .as_ref()
        .map(|a| a.pack_name.clone())
        .unwrap_or_default();

    let article_key = article
        .as_ref()
        .map(|a| format!("{}::{}", a.pack_id, a.article.id))
        .unwrap_or_default();
    if article_key != find_article_key() {
        find_article_key.set(article_key);
        find_draft.set(String::new());
        find_applied.set(String::new());
        find_needle_stamp.set(String::new());
        find_idx.set(0);
        find_hits.set(0);
        find_html.set(None);
    }

    // Apply find against cached body_html (no markdown re-parse). Next/prev only moves focus via JS.
    let applied = find_applied();
    let applied_key = applied.trim().to_string();
    let needs_highlight = !applied_key.is_empty()
        && applied_key.chars().count() >= guides::FIND_MIN_NEEDLE_CHARS;
    if find_needle_stamp() != applied_key {
        find_needle_stamp.set(applied_key.clone());
        find_idx.set(0);
        if let Some(a) = article.as_ref() {
            if needs_highlight {
                let (html, hits) = guides::highlight_article_html(&a.body_html, &applied_key);
                find_html.set(Some(html));
                find_hits.set(hits);
                if hits > 0 {
                    let token = find_scroll_token() + 1;
                    find_scroll_token.set(token);
                    spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                        if find_scroll_token() == token {
                            scroll_guides_find_match(0);
                        }
                    });
                }
            } else {
                find_html.set(None);
                find_hits.set(0);
            }
        } else {
            find_html.set(None);
            find_hits.set(0);
        }
    }
    let body_html = if let Some(html) = find_html() {
        html
    } else if let Some(a) = article.as_ref() {
        a.body_html.clone()
    } else {
        String::new()
    };
    let hits_now = find_hits();
    let idx_now = find_idx();

    let footer = if let Some(a) = article.as_ref() {
        let pack_id = a.pack_id.clone();
        let article_id = a.article.id.clone();
        let pack_md = a.pack_id.clone();
        let art_md = a.article.id.clone();
        let pack_pdf = a.pack_id.clone();
        let art_pdf = a.article.id.clone();
        let siblings: Vec<String> = a.siblings.iter().map(|s| s.id.clone()).collect();
        let idx = siblings.iter().position(|id| id == &article_id);
        let prev = idx.and_then(|i| i.checked_sub(1)).map(|i| siblings[i].clone());
        let next = idx.and_then(|i| {
            let j = i + 1;
            if j < siblings.len() {
                Some(siblings[j].clone())
            } else {
                None
            }
        });
        let pack_prev = pack_id.clone();
        let pack_next = pack_id.clone();
        Some(rsx! {
            div { class: "flex flex-wrap items-center justify-between gap-2",
                div { class: "flex flex-wrap gap-2",
                    Button {
                        kind: ButtonKind::Ghost,
                        disabled: prev.is_none() || loading,
                        onclick: move |_| {
                            if let Some(id) = prev.clone() {
                                on_navigate.call((pack_prev.clone(), id));
                            }
                        },
                        "Previous"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        disabled: next.is_none() || loading,
                        onclick: move |_| {
                            if let Some(id) = next.clone() {
                                on_navigate.call((pack_next.clone(), id));
                            }
                        },
                        "Next"
                    }
                }
                div { class: "flex flex-wrap gap-2",
                    Button {
                        kind: ButtonKind::Ghost,
                        disabled: loading,
                        onclick: move |_| {
                            match guides::export_article_md(&pack_md, &art_md) {
                                Ok(path) => status_note(format!("Saved {}", path.display())),
                                Err(err) if err.contains("cancelled") => {}
                                Err(err) => status_note(err),
                            }
                        },
                        "Export MD"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        disabled: loading,
                        onclick: move |_| {
                            match guides::export_article_pdf(&pack_pdf, &art_pdf) {
                                Ok(path) => status_note(format!("Saved {}", path.display())),
                                Err(err) if err.contains("cancelled") => {}
                                Err(err) => status_note(err),
                            }
                        },
                        "Export PDF"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }
            }
        })
    } else if loading {
        Some(rsx! {
            div { class: "flex justify-end",
                Button {
                    kind: ButtonKind::Ghost,
                    onclick: move |_| on_close.call(()),
                    "Cancel"
                }
            }
        })
    } else {
        None
    };

    let attachments = article
        .as_ref()
        .and_then(|a| guides::load_installed_manifest(&a.pack_id).ok())
        .map(|m| m.attachments)
        .unwrap_or_default();
    let pack_for_att = article.as_ref().map(|a| a.pack_id.clone()).unwrap_or_default();
    let sibling_pills = article
        .as_ref()
        .map(|a| a.siblings.clone())
        .unwrap_or_default();
    let current_id = article
        .as_ref()
        .map(|a| a.article.id.clone())
        .unwrap_or_default();
    let pack_for_pills = article.as_ref().map(|a| a.pack_id.clone()).unwrap_or_default();

    let size_now = font_size();
    let header_actions = if open {
        Some(rsx! {
            div { class: "guides-font-size",
                button {
                    class: if size_now == GuideFontSize::Small {
                        "guides-font-btn guides-font-btn-active"
                    } else {
                        "guides-font-btn"
                    },
                    r#type: "button",
                    title: "Small text",
                    "aria-label": "Small text",
                    onclick: move |_| font_size.set(GuideFontSize::Small),
                    span { class: "guides-font-glyph guides-font-glyph-sm", "A" }
                }
                button {
                    class: if size_now == GuideFontSize::Normal {
                        "guides-font-btn guides-font-btn-active"
                    } else {
                        "guides-font-btn"
                    },
                    r#type: "button",
                    title: "Normal text",
                    "aria-label": "Normal text",
                    onclick: move |_| font_size.set(GuideFontSize::Normal),
                    span { class: "guides-font-glyph guides-font-glyph-md", "A" }
                }
                button {
                    class: if size_now == GuideFontSize::Large {
                        "guides-font-btn guides-font-btn-active"
                    } else {
                        "guides-font-btn"
                    },
                    r#type: "button",
                    title: "Large text",
                    "aria-label": "Large text",
                    onclick: move |_| font_size.set(GuideFontSize::Large),
                    span { class: "guides-font-glyph guides-font-glyph-lg", "A" }
                }
            }
        })
    } else {
        None
    };

    rsx! {
        SlideDownPanel {
            open,
            title,
            subtitle,
            icon: Some(IconName::Book),
            wide: true,
            on_close: move |_| on_close.call(()),
            footer,
            header_actions,
            if open {
                div { class: "guides-reader-chrome",
                    if sibling_pills.len() > 1 {
                        div { class: "guides-article-pills",
                            for sib in sibling_pills.iter() {
                                {
                                    let is_about = sib.id == "about";
                                    let sid = sib.id.clone();
                                    let label = if is_about {
                                        "About".to_string()
                                    } else {
                                        sib.title.clone()
                                    };
                                    let pack = pack_for_pills.clone();
                                    let active = sib.id == current_id;
                                    let kind_cls = if is_about {
                                        "guides-article-pill guides-article-pill-about"
                                    } else {
                                        "guides-article-pill"
                                    };
                                    let class = if active {
                                        format!("{kind_cls} guides-article-pill-active")
                                    } else {
                                        kind_cls.to_string()
                                    };
                                    rsx! {
                                        button {
                                            class: "{class}",
                                            r#type: "button",
                                            title: "{label}",
                                            onclick: move |_| on_navigate.call((pack.clone(), sid.clone())),
                                            "{label}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "guides-find-row",
                        div { class: "guides-find-field",
                            TextInput {
                                value: find_draft(),
                                placeholder: "Find in article…".to_string(),
                                oninput: move |evt: FormEvent| {
                                    // Apply immediately — highlight runs on cached HTML (no debounce lag).
                                    let v = evt.value();
                                    find_draft.set(v.clone());
                                    find_applied.set(v);
                                },
                                onkeydown: move |evt: KeyboardEvent| {
                                    if evt.key() != Key::Enter {
                                        return;
                                    }
                                    let hits = find_hits();
                                    if hits == 0 {
                                        return;
                                    }
                                    evt.prevent_default();
                                    let idx = find_idx();
                                    let next = if evt.modifiers().shift() {
                                        if idx == 0 { hits - 1 } else { idx - 1 }
                                    } else {
                                        (idx + 1) % hits
                                    };
                                    find_idx.set(next);
                                    scroll_guides_find_match(next);
                                },
                            }
                        }
                        if !find_applied().trim().is_empty() {
                            span { class: "guides-find-count",
                                if find_applied().trim().chars().count() < guides::FIND_MIN_NEEDLE_CHARS {
                                    "Type 2+ characters"
                                } else if hits_now == 0 {
                                    "No matches"
                                } else {
                                    "{idx_now + 1} of {hits_now}"
                                }
                            }
                            div { class: "guides-find-nav",
                                button {
                                    class: "guides-find-nav-btn",
                                    r#type: "button",
                                    title: "Previous match (Shift+Enter)",
                                    "aria-label": "Previous match",
                                    disabled: hits_now == 0,
                                    onclick: move |_| {
                                        let hits = find_hits();
                                        if hits == 0 { return; }
                                        let idx = find_idx();
                                        let next = if idx == 0 { hits - 1 } else { idx - 1 };
                                        find_idx.set(next);
                                        scroll_guides_find_match(next);
                                    },
                                    "↑"
                                }
                                button {
                                    class: "guides-find-nav-btn",
                                    r#type: "button",
                                    title: "Next match (Enter)",
                                    "aria-label": "Next match",
                                    disabled: hits_now == 0,
                                    onclick: move |_| {
                                        let hits = find_hits();
                                        if hits == 0 { return; }
                                        let next = (find_idx() + 1) % hits;
                                        find_idx.set(next);
                                        scroll_guides_find_match(next);
                                    },
                                    "↓"
                                }
                            }
                        }
                    }
                }
            }
            if loading && article.is_none() {
                div { class: "flex flex-col items-center justify-center gap-3 py-16 text-muted-foreground",
                    Throbber { size: 22 }
                    p { class: "text-sm", "Loading article…" }
                }
            } else {
                div {
                    class: "{font_size().article_class()}",
                    dangerous_inner_html: "{body_html}",
                }
            }
            if !attachments.is_empty() {
                div { class: "mt-6 border-t border-border pt-3",
                    h3 { class: "mb-2 text-sm font-semibold", "Attachments" }
                    ul { class: "flex flex-col gap-2",
                        for att in attachments.iter() {
                            {
                                let pack = pack_for_att.clone();
                                let path = att.path.clone();
                                let label = att.title.clone();
                                rsx! {
                                    li {
                                        button {
                                            class: "guides-attach-btn",
                                            r#type: "button",
                                            onclick: move |_| {
                                                match guides::open_attachment(&pack, &path) {
                                                    Ok(()) => {}
                                                    Err(err) => status_note(err),
                                                }
                                            },
                                            "Open {label}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            p { class: "mt-6 border-t border-border pt-3 text-xs text-muted-foreground",
                "Reference material only. Verify critical steps with trained local responders when available."
            }
        }
    }
}
