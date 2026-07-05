use vrtb_core::engine::{ColumnSchema, ComparePlan, TableRef};

/// Shared, engine-agnostic: the universal n/v wrapper.
/// NULL  -> 'n'      (presence is encoded in the very first byte)
/// value -> 'v' + payload
/// Every non-null starts with 'v', so no payload can ever be mistaken for the
/// NULL sentinel 'n' — not the empty string, not a literal "n". The wrapper is
/// identical for every type on every engine; only `payload` varies per cell.
pub fn wrap(col: &ColumnSchema, payload: String) -> String {
    format!(
        "CASE WHEN {} IS NULL THEN 'n' ELSE 'v' || {} END",
        quote_ident(&col.name),
        payload
    )
}

pub fn from_table(table: &TableRef) -> String {
    match table.schema {
        Some(ref s) => format!("{}.{}", quote_ident(s), quote_ident(&table.name)),
        None => quote_ident(&table.name),
    }
}

pub fn from_column(table: &TableRef, col: &ColumnSchema) -> String {
    format!("{}.{}", from_table(table), quote_ident(&col.name))
}

pub fn list_columns(table: &TableRef, cols: &[ColumnSchema]) -> String {
    let col_names: Vec<String> = cols.iter().map(|c| from_column(table, c)).collect();
    col_names.join(", ")
}

/// The joindiff output projection: ONLY the key column, qualified by the join
/// alias ("a" or "b") — e.g. `a.id`. check/conformance deliberately emits nothing
/// but row identifiers: the compared columns are referenced solely in the
/// server-side WHERE predicate (see `mismatch_condition`), so no user data ever
/// leaves its database (README §0.3, "no data leaves the user's warehouse").
/// Emitting the differing column *values* is the future `diff` command's job.
pub fn aliased_key(alias: &str, key: &ColumnSchema) -> String {
    format!("{}.{}", alias, quote_ident(&key.name))
}

/// The joindiff/materialize FROM clause: both sides aliased `a`/`b`, FULL OUTER
/// joined USING the key. Shared so the key-streaming and materialize paths can
/// never drift apart on join semantics.
pub fn outer_join_from(a: &TableRef, b: &TableRef, key: &ColumnSchema) -> String {
    format!(
        "FROM {} a FULL OUTER JOIN {} b USING ({})",
        from_table(a),
        from_table(b),
        quote_ident(&key.name)
    )
}

pub fn mismatch_condition(plan: &ComparePlan) -> String {
    // With no compared columns, two key-matched rows can never differ; emit a
    // constant-false predicate so the caller's `AND (...)` stays valid SQL
    // (rather than `AND ()`).
    if plan.columns.is_empty() {
        return "FALSE".to_string();
    }
    let mut conditions: Vec<String> = Vec::new();
    for col in &plan.columns {
        let a_col = format!("a.{}", quote_ident(&col.name));
        let b_col = format!("b.{}", quote_ident(&col.name));
        conditions.push(format!(
            "({a} IS NULL AND {b} IS NOT NULL) OR ({a} IS NOT NULL AND {b} IS NULL) OR ({a} <> {b})",
            a = a_col,
            b = b_col
        ));
    }
    conditions.join(" OR ")
}

/// Quote a SQL identifier for Postgres/DuckDB: wrap in double quotes,
/// escape embedded quotes by doubling. Quoted identifiers are matched
/// case-exactly by Postgres — see note in the README.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrtb_core::engine::LogicalType;

    fn int_col(name: &str) -> ColumnSchema {
        ColumnSchema {
            name: name.into(),
            ty: LogicalType::Int,
            nullable: false,
            default_value: None,
            primary_key: true,
        }
    }

    #[test]
    fn outer_join_from_builds_full_outer_join() {
        let a = TableRef { schema: None, name: "src".into() };
        let b = TableRef { schema: None, name: "dst".into() };
        assert_eq!(
            outer_join_from(&a, &b, &int_col("id")),
            "FROM \"src\" a FULL OUTER JOIN \"dst\" b USING (\"id\")"
        );
    }

    #[test]
    fn quotes_plain_ident() {
        assert_eq!(quote_ident("id"), "\"id\"");
    }

    #[test]
    fn escapes_embedded_quote() {
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn from_table_quotes_schema_and_name_separately() {
        let t = TableRef {
            schema: Some("public".into()),
            name: "customers".into(),
        };
        assert_eq!(from_table(&t), "\"public\".\"customers\"");
    }
}
