use servo::profile_traits::mem::{MemoryReportResult, ReportKind};
use std::collections::BTreeMap;

const SHOWN: usize = 30;
const LONGEST_NAME: usize = 120;

fn megabytes(bytes: usize) -> f64 {
    (bytes as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0
}

fn name(path: &[String]) -> String {
    let joined = if path.get(1).map(String::as_str) == Some("js")
        && path.get(2).map(String::as_str) == Some("dom")
        && path.get(3).map(String::as_str) == Some("out-of-tree")
        && let Some(interface) = path.get(5)
    {
        if path.get(4).is_some_and(|global| global.contains("-zombie")) {
            format!(
                "{} / js / dom / out-of-tree / zombie / {interface}",
                path[0]
            )
        } else {
            format!("{} / js / dom / out-of-tree / {interface}", path[0])
        }
    } else {
        let take = if path.get(1).map(String::as_str) == Some("js")
            && path.get(2).map(String::as_str) == Some("dom")
        {
            4
        } else {
            3
        };
        path.iter()
            .take(take)
            .cloned()
            .collect::<Vec<_>>()
            .join(" / ")
    };
    if joined.chars().count() > LONGEST_NAME {
        let cut: String = joined.chars().take(LONGEST_NAME).collect();
        format!("{cut}…")
    } else {
        joined
    }
}

pub fn summary(result: &MemoryReportResult) -> String {
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut overall: Vec<(String, usize)> = Vec::new();
    let mut explicit = 0;
    for report in result
        .results
        .iter()
        .flat_map(|process| process.reports.iter())
    {
        match report.kind {
            ReportKind::NonExplicitSize => overall.push((report.path.join(" / "), report.size)),
            _ => {
                explicit += report.size;
                *groups.entry(name(&report.path)).or_default() += report.size;
            }
        }
    }
    let mut largest: Vec<(String, usize)> = groups.into_iter().collect();
    largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut lines = vec![format!(
        "Memory report: {} MB measured by Servo",
        megabytes(explicit)
    )];
    overall.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, size) in overall {
        lines.push(format!("  {:>8} MB  {path}", megabytes(size)));
    }
    lines.push("Largest parts:".to_string());
    for (path, size) in largest.into_iter().take(SHOWN) {
        lines.push(format!("  {:>8} MB  {path}", megabytes(size)));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use servo::profile_traits::mem::{MemoryReport, Report};

    fn report(path: &[&str], kind: ReportKind, size: usize) -> Report {
        Report {
            path: path.iter().map(|part| part.to_string()).collect(),
            kind,
            size,
        }
    }

    #[test]
    fn sums_up_the_largest_parts() {
        let result = MemoryReportResult {
            results: vec![MemoryReport {
                pid: 1,
                is_main_process: true,
                reports: vec![
                    report(
                        &["resident"],
                        ReportKind::NonExplicitSize,
                        300 * 1024 * 1024,
                    ),
                    report(
                        &["page", "js", "gc-heap", "objects"],
                        ReportKind::ExplicitJemallocHeapSize,
                        40 * 1024 * 1024,
                    ),
                    report(
                        &["page", "js", "gc-heap", "strings"],
                        ReportKind::ExplicitJemallocHeapSize,
                        20 * 1024 * 1024,
                    ),
                    report(
                        &["image-cache"],
                        ReportKind::ExplicitJemallocHeapSize,
                        30 * 1024 * 1024,
                    ),
                ],
            }],
        };
        let text = summary(&result);
        assert!(text.starts_with("Memory report: 90 MB measured by Servo"));
        assert!(text.contains("300 MB  resident"));
        let js = text.find("60 MB  page / js / gc-heap").unwrap();
        let images = text.find("30 MB  image-cache").unwrap();
        assert!(js < images);
    }

    #[test]
    fn shortens_long_names() {
        let long = "x".repeat(200);
        assert!(name(&[long]).ends_with('…'));
        assert_eq!(megabytes(1536 * 1024), 1.5);
    }

    #[test]
    fn separates_in_tree_and_out_of_tree_dom_reports() {
        let result = MemoryReportResult {
            results: vec![MemoryReport {
                pid: 1,
                is_main_process: true,
                reports: vec![
                    report(
                        &["url(x)", "js", "dom", "element-nodes"],
                        ReportKind::ExplicitJemallocHeapSize,
                        4 * 1024 * 1024,
                    ),
                    report(
                        &[
                            "url(x)",
                            "js",
                            "dom",
                            "out-of-tree",
                            "url(x)-1",
                            "HTMLDivElement [10]",
                        ],
                        ReportKind::ExplicitJemallocHeapSize,
                        12 * 1024 * 1024,
                    ),
                    report(
                        &[
                            "url(x)",
                            "js",
                            "dom",
                            "out-of-tree",
                            "url(x)-zombie-1",
                            "Window [1]",
                        ],
                        ReportKind::ExplicitJemallocHeapSize,
                        2 * 1024 * 1024,
                    ),
                    report(
                        &["url(x)", "js", "gc-heap", "objects"],
                        ReportKind::ExplicitNonHeapSize,
                        6 * 1024 * 1024,
                    ),
                    report(
                        &["url(x)", "js", "gc-heap", "strings"],
                        ReportKind::ExplicitNonHeapSize,
                        2 * 1024 * 1024,
                    ),
                ],
            }],
        };
        let text = summary(&result);
        assert!(text.contains("4 MB  url(x) / js / dom / element-nodes"));
        assert!(text.contains("12 MB  url(x) / js / dom / out-of-tree / HTMLDivElement [10]"));
        assert!(text.contains("2 MB  url(x) / js / dom / out-of-tree / zombie / Window [1]"));
        assert!(text.contains("8 MB  url(x) / js / gc-heap"));
    }
}
