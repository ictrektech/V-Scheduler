//! 列表分页参数与响应元数据。
//!
//! 契约见 `openspec/changes/modelinfer-list-pagination/design.md` §2：
//! - 请求：`page_num`（1-based，缺省 1）/ `page_size`（缺省按端点，`clamp(1, 100)`）；
//! - 响应：在 `data` 数组之外返回 `total`（**筛选后**总数）/ `pages` / `page_num` / `page_size`，
//!   字段名与平台 `Pagination<T>` 对齐。
//!
//! 本模块只做参数归一化与切片，**不含**数据库 / 领域逻辑，便于单测。

use serde::{Deserialize, Serialize};

/// 表格类列表默认页大小
pub const DEFAULT_PAGE_SIZE: u64 = 20;
/// 卡片网格类列表默认页大小（4 列 × 6 行）
pub const GRID_PAGE_SIZE: u64 = 24;
/// 页大小上限（防恶意大查询）
pub const MAX_PAGE_SIZE: u64 = 100;

/// 分页查询参数（所有字段可选，缺省即第 1 页 + 端点默认页大小）
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    pub page_num: Option<u64>,
    #[serde(default)]
    pub page_size: Option<u64>,
}

impl PageQuery {
    /// 按端点默认页大小归一化为可直接使用的分页参数
    pub fn resolve(&self, default_size: u64) -> PageParams {
        PageParams {
            page_num: normalize_page(self.page_num),
            page_size: normalize_page_size(self.page_size, default_size),
        }
    }
}

/// 归一化后的分页参数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageParams {
    pub page_num: u64,
    pub page_size: u64,
}

impl PageParams {
    /// 零基起始下标
    pub fn offset(&self) -> usize {
        ((self.page_num.saturating_sub(1)) * self.page_size) as usize
    }
}

/// 分页响应元数据（以 `#[serde(flatten)]` 展开到各列表 VO 中）
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PageMeta {
    /// **筛选后**总条数
    pub total: usize,
    /// 总页数（最小 1）
    pub pages: u64,
    pub page_num: u64,
    pub page_size: u64,
}

/// 页码归一化：缺省或 0 视为第 1 页
pub fn normalize_page(page_num: Option<u64>) -> u64 {
    page_num.filter(|v| *v > 0).unwrap_or(1)
}

/// 页大小归一化：缺省 / 0 取端点默认值，超过上限截断
pub fn normalize_page_size(page_size: Option<u64>, default_size: u64) -> u64 {
    match page_size {
        Some(v) if v > 0 => v.min(MAX_PAGE_SIZE),
        _ => default_size.min(MAX_PAGE_SIZE),
    }
}

/// 总页数（最小 1；`page_size` 为 0 时视为 1 页）
pub fn pages_of(total: usize, page_size: u64) -> u64 {
    if page_size == 0 {
        return 1;
    }
    let pages = (total as u64).div_ceil(page_size);
    pages.max(1)
}

/// 对**已筛选 / 已排序**的列表切片，返回当前页数据与分页元数据。
///
/// 越界页返回空 `data`（`total` / `pages` 仍反映真实总数），便于前端在删除末页条目后
/// 自行回退页码时不会报错。
pub fn paginate<T>(items: Vec<T>, params: PageParams) -> (Vec<T>, PageMeta) {
    let total = items.len();
    let pages = pages_of(total, params.page_size);
    let start = params.offset();
    let data: Vec<T> = items
        .into_iter()
        .skip(start)
        .take(params.page_size as usize)
        .collect();
    (
        data,
        PageMeta {
            total,
            pages,
            page_num: params.page_num,
            page_size: params.page_size,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_query_normalizes_invalid_values() {
        let q = PageQuery { page_num: Some(0), page_size: Some(0) };
        let p = q.resolve(DEFAULT_PAGE_SIZE);
        assert_eq!(p.page_num, 1);
        assert_eq!(p.page_size, DEFAULT_PAGE_SIZE);

        let q = PageQuery { page_num: None, page_size: Some(10_000) };
        let p = q.resolve(GRID_PAGE_SIZE);
        assert_eq!(p.page_num, 1);
        assert_eq!(p.page_size, MAX_PAGE_SIZE);

        let q = PageQuery::default();
        let p = q.resolve(GRID_PAGE_SIZE);
        assert_eq!(p.page_num, 1);
        assert_eq!(p.page_size, GRID_PAGE_SIZE);
    }

    #[test]
    fn pages_are_at_least_one() {
        assert_eq!(pages_of(0, 20), 1);
        assert_eq!(pages_of(1, 20), 1);
        assert_eq!(pages_of(20, 20), 1);
        assert_eq!(pages_of(21, 20), 2);
        assert_eq!(pages_of(41, 20), 3);
        assert_eq!(pages_of(5, 0), 1);
    }

    #[test]
    fn paginate_slices_expected_page() {
        let items: Vec<u32> = (1..=25).collect();
        let (page1, meta) = paginate(items.clone(), PageParams { page_num: 1, page_size: 10 });
        assert_eq!(page1, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(meta.total, 25);
        assert_eq!(meta.pages, 3);
        assert_eq!(meta.page_num, 1);
        assert_eq!(meta.page_size, 10);

        let (page3, meta3) = paginate(items.clone(), PageParams { page_num: 3, page_size: 10 });
        assert_eq!(page3, vec![21, 22, 23, 24, 25]);
        assert_eq!(meta3.pages, 3);
    }

    #[test]
    fn paginate_out_of_range_returns_empty_page() {
        let items: Vec<u32> = (1..=5).collect();
        let (data, meta) = paginate(items, PageParams { page_num: 9, page_size: 10 });
        assert!(data.is_empty());
        assert_eq!(meta.total, 5);
        assert_eq!(meta.pages, 1);
    }

    #[test]
    fn paginate_empty_list() {
        let (data, meta) = paginate(Vec::<u32>::new(), PageParams { page_num: 1, page_size: 20 });
        assert!(data.is_empty());
        assert_eq!(meta.total, 0);
        assert_eq!(meta.pages, 1);
    }
}