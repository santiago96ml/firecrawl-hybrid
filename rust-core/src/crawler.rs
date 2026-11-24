use crate::ai::extract_from_markdown;
use crate::models::{CrawlJob, CrawlRequest, ScrapeRequest};
use crate::scraper::scrape_url_base;
use crate::AppState;
use chrono::{Duration as ChronoDuration, Utc};
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use tracing::{info, warn};
use url::Url;

pub fn spawn_crawl_job(state: AppState, job_id: String, req: CrawlRequest) {
    let expires_at = (Utc::now() + ChronoDuration::hours(24)).to_rfc3339();
    let initial_job = CrawlJob {
        id: job_id.clone(),
        status: "scraping".to_string(),
        total: 1,
        completed: 0,
        credits_used: 0,
        expires_at,
        data: Vec::new(),
        error: None,
    };
    state.crawl_jobs.insert(job_id.clone(), initial_job);

    tokio::spawn(async move {
        run_bfs_crawl(state, job_id, req).await;
    });
}

async fn run_bfs_crawl(state: AppState, job_id: String, req: CrawlRequest) {
    let limit = req.limit.unwrap_or(25).clamp(1, 500);
    let max_depth = req
        .max_discovery_depth
        .or(req.max_depth)
        .unwrap_or(3)
        .clamp(0, 10);
    let allow_external = req.allow_external_links.unwrap_or(false);

    let root_url = match Url::parse(&req.url) {
        Ok(u) => u,
        Err(err) => {
            if let Some(mut job) = state.crawl_jobs.get_mut(&job_id) {
                job.status = "failed".to_string();
                job.error = Some(format!("Invalid root URL: {}", err));
            }
            return;
        }
    };
    let root_host = root_url.host_str().unwrap_or_default().to_lowercase();

    let include_regexes = compile_patterns(req.include_paths.as_deref().unwrap_or(&[]));
    let exclude_regexes = compile_patterns(req.exclude_paths.as_deref().unwrap_or(&[]));

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    let start_norm = normalize_url(&req.url);
    visited.insert(start_norm.clone());
    queue.push_back((req.url.clone(), 0));

    info!(
        "Starting BFS crawl job {} on {} (limit={}, maxDepth={})",
        job_id, req.url, limit, max_depth
    );

    while !queue.is_empty() {
        // Check if job was cancelled
        if let Some(job) = state.crawl_jobs.get(&job_id) {
            if job.status == "cancelled" {
                info!("Crawl job {} cancelled by user", job_id);
                return;
            }
            if job.completed >= limit {
                break;
            }
        } else {
            return;
        }

        // Collect a batch of URLs up to remaining limit and concurrency window
        let current_completed = state
            .crawl_jobs
            .get(&job_id)
            .map(|j| j.completed)
            .unwrap_or(0);
        if current_completed >= limit {
            break;
        }
        let remaining = limit - current_completed;
        let batch_size = remaining.min(10).min(queue.len());
        if batch_size == 0 {
            break;
        }

        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            if let Some(item) = queue.pop_front() {
                batch.push(item);
            }
        }

        // Update total discovered count
        if let Some(mut job) = state.crawl_jobs.get_mut(&job_id) {
            job.total = (job.completed + batch.len() + queue.len()).min(limit);
        }

        let mut handles = Vec::with_capacity(batch.len());
        for (target_url, depth) in batch {
            let st = state.clone();
            let opts = req.scrape_options.clone();
            handles.push(tokio::spawn(async move {
                let _permit = st.crawl_semaphore.acquire().await.ok();
                let sreq = ScrapeRequest {
                    url: target_url.clone(),
                    formats: opts.as_ref().and_then(|o| o.formats.clone()),
                    only_main_content: opts.as_ref().and_then(|o| o.only_main_content),
                    include_tags: opts.as_ref().and_then(|o| o.include_tags.clone()),
                    exclude_tags: opts.as_ref().and_then(|o| o.exclude_tags.clone()),
                    headers: opts.as_ref().and_then(|o| o.headers.clone()),
                    wait_for: None,
                    timeout: opts.as_ref().and_then(|o| o.timeout).or(Some(20_000)),
                    extract: opts.as_ref().and_then(|o| o.extract.clone()),
                    json_options: opts.as_ref().and_then(|o| o.json_options.clone()),
                };
                let mut data = scrape_url_base(&st, &sreq).await;

                // Optional AI extraction per crawled page
                let wants_extract = sreq
                    .formats
                    .as_ref()
                    .map(|f| {
                        f.iter()
                            .any(|x| x.eq_ignore_ascii_case("extract") || x.eq_ignore_ascii_case("json"))
                    })
                    .unwrap_or(false)
                    || sreq.extract.is_some()
                    || sreq.json_options.is_some();

                if wants_extract {
                    let ext_opts = sreq.extract.as_ref().or(sreq.json_options.as_ref());
                    let extracted = extract_from_markdown(
                        &st,
                        &data.markdown,
                        ext_opts.and_then(|e| e.prompt.as_deref()),
                        ext_opts.and_then(|e| e.system_prompt.as_deref()),
                        ext_opts.and_then(|e| e.schema.as_ref()),
                    )
                    .await;
                    data.extract = Some(extracted.clone());
                    data.json = Some(extracted);
                }

                (data, depth)
            }));
        }

        for h in handles {
            match h.await {
                Ok((page_data, depth)) => {
                    // Discover child links if within depth limit
                    if depth < max_depth {
                        for link in &page_data.links {
                            if visited.len() >= limit * 3 {
                                break;
                            }
                            let norm = normalize_url(link);
                            if visited.contains(&norm) {
                                continue;
                            }
                            if should_follow_link(
                                link,
                                &root_host,
                                allow_external,
                                &include_regexes,
                                &exclude_regexes,
                            ) {
                                visited.insert(norm);
                                queue.push_back((link.clone(), depth + 1));
                            }
                        }
                    }

                    if let Some(mut job) = state.crawl_jobs.get_mut(&job_id) {
                        if job.status == "cancelled" {
                            return;
                        }
                        if job.completed < limit {
                            job.data.push(page_data);
                            job.completed = job.data.len();
                            job.credits_used = job.completed;
                            job.total = (job.completed + queue.len()).min(limit).max(job.completed);
                        }
                    }
                }
                Err(err) => {
                    warn!("Crawl task join error in job {}: {}", job_id, err);
                }
            }
        }
    }

    if let Some(mut job) = state.crawl_jobs.get_mut(&job_id) {
        if job.status != "cancelled" {
            job.status = "completed".to_string();
            job.total = job.completed;
            info!(
                "Completed crawl job {} with {} pages",
                job_id, job.completed
            );
        }
    }
}

fn normalize_url(raw: &str) -> String {
    if let Ok(mut u) = Url::parse(raw) {
        u.set_fragment(None);
        let mut s = u.to_string();
        if s.ends_with('/') && s.len() > 8 {
            s.pop();
        }
        s
    } else {
        raw.trim_end_matches('/').to_string()
    }
}

fn compile_patterns(patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|p| {
            Regex::new(p).ok().or_else(|| {
                let escaped = regex::escape(p).replace(r"\*", ".*");
                Regex::new(&escaped).ok()
            })
        })
        .collect()
}

fn should_follow_link(
    link: &str,
    root_host: &str,
    allow_external: bool,
    include_regexes: &[Regex],
    exclude_regexes: &[Regex],
) -> bool {
    let Ok(parsed) = Url::parse(link) else {
        return false;
    };
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return false;
    }

    if !allow_external {
        let host = parsed.host_str().unwrap_or_default().to_lowercase();
        if host != root_host {
            return false;
        }
    }

    let path = parsed.path();
    for ex in exclude_regexes {
        if ex.is_match(path) || ex.is_match(link) {
            return false;
        }
    }

    if !include_regexes.is_empty() {
        let matched = include_regexes
            .iter()
            .any(|inc| inc.is_match(path) || inc.is_match(link));
        if !matched {
            return false;
        }
    }

    true
}
