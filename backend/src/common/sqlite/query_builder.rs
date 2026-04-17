/// Helper to build dynamic WHERE clauses with filter parameters.
///
/// Usage:
/// ```ignore
/// let mut q = PageQuery::new();
/// q.pagination(req.current, req.size);
/// q.add_text_like(req.username.as_ref(), "u.username");
/// q.add_text_eq(req.org_id.as_ref(), "u.org_id");
/// q.add_number_eq(req.enable, "u.enable");
///
/// let where_clause = q.where_clause("u.is_deleted = 0");
///
/// // For count:
/// let total = bind_filters!(query_scalar(&count_sql), q.filters).fetch_one(&pool).await?;
///
/// // For records:
/// let rows = bind_filters!(query(&records_sql), q.filters, q.size, q.offset)
///     .fetch_all(&pool).await?;
/// ```
pub struct PageQuery {
    pub conditions: Vec<String>,
    pub filters: Vec<FilterValue>,
    pub size: i64,
    pub offset: i64,
}

pub enum FilterValue {
    Text(String),
    Number(i32),
}

impl Default for PageQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl PageQuery {
    pub fn new() -> Self {
        Self {
            conditions: Vec::new(),
            filters: Vec::new(),
            size: 10,
            offset: 0,
        }
    }

    pub fn add_text_like(&mut self, value: Option<&String>, column: &str) {
        if let Some(v) = value {
            self.conditions.push(format!("{column} LIKE ?"));
            self.filters.push(FilterValue::Text(format!("%{}%", v)));
        }
    }

    pub fn add_text_eq(&mut self, value: Option<&String>, column: &str) {
        if let Some(v) = value {
            self.conditions.push(format!("{column} = ?"));
            self.filters.push(FilterValue::Text(v.clone()));
        }
    }

    pub fn add_number_eq(&mut self, value: Option<i32>, column: &str) {
        if let Some(v) = value {
            self.conditions.push(format!("{column} = ?"));
            self.filters.push(FilterValue::Number(v));
        }
    }

    pub fn pagination(&mut self, current: i64, size: i64) {
        let page = current.max(1);
        let page_size = size.max(1);
        self.size = page_size;
        self.offset = (page - 1) * page_size;
    }

    pub fn where_clause(&self, base: &str) -> String {
        if self.conditions.is_empty() {
            base.to_string()
        } else {
            format!("{} AND {}", base, self.conditions.join(" AND "))
        }
    }
}

/// Bind filters to a query_scalar, returning the bound query.
/// Usage: `bind_filters!(query_scalar(&sql), q.filters)`
#[macro_export]
macro_rules! bind_filters {
    ($q:expr, $filters:expr) => {{
        let mut __q = $q;
        for __f in &$filters {
            __q = match __f {
                $crate::common::sqlite::query_builder::FilterValue::Text(__v) => __q.bind(__v),
                $crate::common::sqlite::query_builder::FilterValue::Number(__v) => __q.bind(__v),
            };
        }
        __q
    }};
    ($q:expr, $filters:expr, $size:expr, $offset:expr) => {{
        let mut __q = $q;
        for __f in &$filters {
            __q = match __f {
                $crate::common::sqlite::query_builder::FilterValue::Text(__v) => __q.bind(__v),
                $crate::common::sqlite::query_builder::FilterValue::Number(__v) => __q.bind(__v),
            };
        }
        __q.bind($size).bind($offset)
    }};
}
