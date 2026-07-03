use vrtb_core::engine::{ColumnSchema, ComparePlan, TableRef};

// Shared, engine-agnostic: the universal n/v wrapper.
// NULL  -> 'n'      (presence is encoded in the very first byte)
// value -> 'v' + payload
// Every non-null starts with 'v', so no payload can ever be mistaken for the
// NULL sentinel 'n' — not the empty string, not a literal "n". The wrapper is
// identical for every type on every engine; only `payload` varies per cell.
pub fn wrap(col: &ColumnSchema, payload: String) -> String {
    format!(
        "CASE WHEN {} IS NULL THEN 'n' ELSE 'v' || {} END",
        col.name, payload
    )
}

pub fn from_table(table: &TableRef) -> String {
    match table.schema {
        Some(ref s) => format!("{}.{}", s, table.name),
        None => table.name.clone(),
    }
}

pub fn from_column(table: &TableRef, col: &ColumnSchema) -> String {
    format!("{}.{}", from_table(table), col.name)
}

pub fn list_columns(table: &TableRef, cols: &[ColumnSchema]) -> String {
    let col_names: Vec<String> = cols.iter().map(|c| from_column(table, c)).collect();
    col_names.join(", ")
}

// The joindiff output projection: ONLY the key column, qualified by the join
// alias ("a" or "b") — e.g. `a.id`. check/conformance deliberately emits nothing
// but row identifiers: the compared columns are referenced solely in the
// server-side WHERE predicate (see `mimatch_condition`), so no user data ever
// leaves its database (README §0.3, "no data leaves the user's warehouse").
// Emitting the differing column *values* is the future `diff` command's job.
pub fn aliased_key(alias: &str, key: &ColumnSchema) -> String {
    format!("{}.{}", alias, key.name)
}

pub fn mimatch_condition(plan: &ComparePlan) -> String {
    // With no compared columns, two key-matched rows can never differ; emit a
    // constant-false predicate so the caller's `AND (...)` stays valid SQL
    // (rather than `AND ()`).
    if plan.columns.is_empty() {
        return "FALSE".to_string();
    }
    let mut conditions: Vec<String> = Vec::new();
    for col in &plan.columns {
        let a_col = format!("a.{}", col.name);
        let b_col = format!("b.{}", col.name);
        conditions.push(format!(
            "({a} IS NULL AND {b} IS NOT NULL) OR ({a} IS NOT NULL AND {b} IS NULL) OR ({a} <> {b})",
            a = a_col,
            b = b_col
        ));
    }
    conditions.join(" OR ")
}
