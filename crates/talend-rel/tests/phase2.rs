//! EKOS RFC 0267 phase 2 on small jobs written for the purpose: ELT maps, statements, `tJavaRow`
//! and run-time context, `Numeric.sequence`, `tSampleRow`.

use ekos_lakehouse_etl_expr::{Expr, Fidelity, Func};
use ekos_lakehouse_etl_rel::{Ir, JoinKind, NoCatalog, Op, RelNode};
use ekos_lakehouse_talend_rel::{Lowering, lower_job};
use ekos_talend::{ProjectFile, read_project};

fn node(kind: &str, name: &str, extra: &str) -> String {
    format!(
        r#"<node componentName="{kind}"><elementParameter field="TEXT" name="UNIQUE_NAME" value="{name}"/>{extra}</node>"#
    )
}

fn meta(cols: &[(&str, &str)]) -> String {
    let cols: String = cols
        .iter()
        .map(|(c, t)| format!(r#"<column name="{c}" type="{t}"/>"#))
        .collect();
    format!(r#"<metadata connector="FLOW" name="x">{cols}</metadata>"#)
}

fn strings(cols: &[&str]) -> String {
    meta(&cols.iter().map(|c| (*c, "id_String")).collect::<Vec<_>>())
}

fn param(name: &str, value: &str) -> String {
    let v = value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(r#"<elementParameter field="TEXT" name="{name}" value="{v}"/>"#)
}

fn table(name: &str, rows: &[&[(&str, &str)]]) -> String {
    let values: String = rows
        .iter()
        .flat_map(|r| *r)
        .map(|(k, v)| {
            let v = v.replace('&', "&amp;").replace('"', "&quot;");
            format!(r#"<elementValue elementRef="{k}" value="{v}"/>"#)
        })
        .collect();
    format!(r#"<elementParameter field="TABLE" name="{name}">{values}</elementParameter>"#)
}

fn link(label: &str, from: &str, to: &str) -> String {
    format!(
        r#"<connection connectorName="FLOW" label="{label}" metaname="{label}" source="{from}" target="{to}"/>"#
    )
}

fn table_link(label: &str, meta: &str, from: &str, to: &str) -> String {
    format!(
        r#"<connection connectorName="TABLE" label="{label}" lineStyle="9" metaname="{meta}" source="{from}" target="{to}"/>"#
    )
}

fn lowered(body: &str, context: &str) -> Lowering {
    let xml = format!(
        r#"<talendfile:ProcessType xmlns:talendfile="platform:/resource/org.talend.model/model/TalendFile.xsd" defaultContext="Default"><context name="Default">{context}</context>{body}</talendfile:ProcessType>"#
    );
    let p = read_project(
        "",
        &[ProjectFile {
            path: "process/j_0.1.item".into(),
            text: xml,
        }],
    );
    let job = p.expand(&p.jobs[0]).job;
    lower_job(&job, &p.routines, &NoCatalog)
}

fn of<'a>(ir: &'a Ir, step: &str) -> Vec<&'a RelNode> {
    ir.nodes.iter().filter(|n| n.step == step).collect()
}

fn no_gaps(l: &Lowering) {
    for n in &l.ir.nodes {
        assert!(!n.is_gap(), "{}: {:?}", n.step, n.op);
    }
}

#[test]
fn an_elt_map_joins_filters_groups_and_projects() {
    let input = |name: &str, table: &str, cols: &[&str]| {
        node(
            "tELTInput",
            name,
            &format!(
                "{}{}{}",
                param("ELT_TABLE_NAME", &format!("\"{table}\"")),
                param("ELT_SCHEMA_NAME", "\"STG\""),
                strings(cols)
            ),
        )
    };
    let output = |name: &str, cols: &[&str]| {
        node(
            "tELTOutput",
            name,
            &format!(
                "{}{}{}",
                param("ELT_TABLE_NAME", "\"F\""),
                param("ELT_SCHEMA_NAME", "\"DWH\""),
                strings(cols)
            ),
        )
    };
    let map = node(
        "tELTMap",
        "tELTMap_1",
        r#"<elementParameter field="MAPPING_TYPE" name="MAPPING" value="snowflake_id"/><nodeData xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="TalendDBMap:DBMapData">
          <InputTables alias="A" joinType="NO_JOIN" name="A" tableName="STG.S">
            <DBMapperTableEntries name="ID"/><DBMapperTableEntries name="NAME"/><DBMapperTableEntries name="QTY"/>
          </InputTables>
          <InputTables alias="B" joinType="LEFT_OUTER_JOIN" name="B" tableName="STG.D">
            <DBMapperTableEntries expression="A.ID" join="true" name="ID" operator="="/>
            <DBMapperTableEntries expression="'X'" join="true" name="KIND" operator="="/>
            <DBMapperTableEntries name="PK"/>
          </InputTables>
          <OutputTables name="agg" tableName="F">
            <DBMapperTableEntries expression="upper(A.NAME)" name="NAME"/>
            <DBMapperTableEntries expression="COALESCE(sum(A.QTY), 0)" name="TOTAL"/>
            <DBMapperTableEntries expression="HASH(TO_VARIANT(A.NAME))" name="PK"/>
            <FilterEntries expression="GROUP BY A.NAME" filterKind="OTHER_FILTER" name="f1"/>
          </OutputTables>
          <OutputTables name="known" tableName="F">
            <DBMapperTableEntries expression="B.PK" name="PK"/>
            <FilterEntries expression="B.PK is not null" filterKind="WHERE_FILTER" name="f2"/>
          </OutputTables>
        </nodeData>"#,
    );
    let body = [
        input("tELTInput_1", "S", &["ID", "NAME", "QTY"]),
        input("tELTInput_2", "D", &["ID", "KIND", "PK"]),
        map,
        output("tELTOutput_1", &["NAME", "TOTAL", "PK"]),
        output("tELTOutput_2", &["PK"]),
        table_link("STG.S", "t1", "tELTInput_1", "tELTMap_1"),
        table_link("STG.D", "t2", "tELTInput_2", "tELTMap_1"),
        table_link("F", "agg", "tELTMap_1", "tELTOutput_1"),
        table_link("F", "known", "tELTMap_1", "tELTOutput_2"),
    ]
    .concat();
    let l = lowered(&body, "");
    no_gaps(&l);
    let map = of(&l.ir, "tELTMap_1");
    // The literal key filters the joined input; the column key is a join key.
    assert!(
        map.iter().any(
            |n| matches!(&n.op, Op::Filter { predicate } if predicate.sql().contains("B__KIND"))
        )
    );
    let join = map
        .iter()
        .find_map(|n| match &n.op {
            Op::Join { kind, keys, .. } => Some((*kind, keys.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        join,
        (JoinKind::Left, vec![("A__ID".into(), "B__ID".into())])
    );
    let (group_by, aggs) = map
        .iter()
        .find_map(|n| match &n.op {
            Op::Aggregate { group_by, aggs } => Some((group_by.clone(), aggs.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(group_by, ["__g0"]);
    assert_eq!(aggs[0].func, "sum");
    let out = map
        .iter()
        .rev()
        .find(|n| n.branch.as_deref() == Some("agg") && matches!(n.op, Op::Project { .. }))
        .unwrap();
    let Op::Project { items } = &out.op else {
        unreachable!()
    };
    let sql = |name: &str| {
        items
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, e)| e.sql())
            .unwrap()
    };
    assert!(sql("NAME").contains("__g0"), "{}", sql("NAME"));
    assert!(sql("TOTAL").contains("__agg_0"), "{}", sql("TOTAL"));
    assert!(sql("PK").contains("md5"), "{}", sql("PK"));
    assert_eq!(out.fidelity, Fidelity::Rewritten);
    assert!(out.notes.iter().any(|n| n.contains("HASH")));
    // Each output feeds its tELTOutput.
    let sink = of(&l.ir, "tELTOutput_1")[0];
    assert_eq!(sink.inputs, [out.id]);
}

#[test]
fn a_row_statement_is_a_statement_node_with_its_set_and_where() {
    let props = r#"{"query":{"storedValue":"\"update DB.CNF.PROCESS_CONTROL set LAST = CURRENT_TIMESTAMP() where SOURCE = 'SAP'\"","name":"query"}}"#;
    let body = node(
        "tSnowflakeRow",
        "tDBRow_1",
        &format!(
            r#"<elementParameter field="TECHNICAL" name="PROPERTIES" value="{}"/>"#,
            props.replace('"', "&quot;")
        ),
    );
    let l = lowered(&body, "");
    no_gaps(&l);
    let n = of(&l.ir, "tDBRow_1")[0];
    let Op::Statement {
        verb,
        target,
        set,
        predicate,
        ..
    } = &n.op
    else {
        panic!("{:?}", n.op)
    };
    assert_eq!(verb, "UPDATE");
    let t = target.as_ref().unwrap();
    assert_eq!(
        (t.schema.as_deref(), t.table.as_str()),
        (Some("CNF"), "PROCESS_CONTROL")
    );
    assert_eq!(set[0].0, "LAST");
    assert!(matches!(
        &set[0].1,
        Expr::Call {
            func: Func::Now,
            ..
        }
    ));
    assert!(predicate.as_ref().unwrap().sql().contains("'SAP'"));
    assert_eq!(n.fidelity, Fidelity::Rewritten);
}

#[test]
fn a_context_set_from_a_row_makes_every_query_that_reads_it_rewritten() {
    let body = [
        node(
            "tOracleInput",
            "tDBInput_2",
            &format!(
                "{}{}",
                param("QUERY", "\"SELECT max(D) AS LAST_LOADED_DATE FROM CNF.W\""),
                strings(&["LAST_LOADED_DATE"])
            ),
        ),
        node(
            "tJavaRow",
            "tJavaRow_1",
            &param(
                "CODE",
                "context.last_loaded_date = input_row.LAST_LOADED_DATE;\nSystem.out.println(\"Last: \" + context.last_loaded_date);",
            ),
        ),
        node(
            "tOracleInput",
            "tDBInput_1",
            &format!(
                "{}{}",
                param(
                    "QUERY",
                    "\"SELECT A FROM S.T WHERE D >= '\"+context.last_loaded_date+\"'\""
                ),
                strings(&["A"])
            ),
        ),
        link("row1", "tDBInput_2", "tJavaRow_1"),
    ]
    .concat();
    let l = lowered(
        &body,
        r#"<contextParameter name="last_loaded_date" type="id_String" value="2020-01-01"/>"#,
    );
    no_gaps(&l);
    assert_eq!(l.context_from_data.len(), 1);
    let w = &l.context_from_data[0];
    assert_eq!(
        (w.var.as_str(), w.component.as_str(), w.column.as_str()),
        ("last_loaded_date", "tJavaRow_1", "LAST_LOADED_DATE")
    );
    let read = of(&l.ir, "tDBInput_1")[0];
    assert_eq!(read.fidelity, Fidelity::Rewritten);
    assert!(
        read.notes
            .iter()
            .any(|n| n.contains("tJavaRow_1") && n.contains("watermark")),
        "{:?}",
        read.notes
    );
    let source = of(&l.ir, "tDBInput_2")[0];
    assert_eq!(source.fidelity, Fidelity::Exact);
}

#[test]
fn a_java_row_assigns_its_outputs_and_leaves_the_rest_null() {
    let body = [
        node(
            "tFileInputDelimited",
            "tFileInputDelimited_1",
            &format!("{}{}", param("FILENAME", "\"/in/a.csv\""), strings(&["A", "B"])),
        ),
        node(
            "tJavaRow",
            "tJavaRow_1",
            &format!(
                "{}{}",
                param(
                    "CODE",
                    "if (input_row.B == null) {\n  System.out.println(\"no B\");\n}\noutput_row.A = input_row.A.toUpperCase();"
                ),
                strings(&["A", "B"])
            ),
        ),
        node("tLogRow", "tLogRow_1", &strings(&["A", "B"])),
        link("row1", "tFileInputDelimited_1", "tJavaRow_1"),
        link("row2", "tJavaRow_1", "tLogRow_1"),
    ]
    .concat();
    let l = lowered(&body, "");
    no_gaps(&l);
    let n = of(&l.ir, "tJavaRow_1")[0];
    let Op::Project { items } = &n.op else {
        panic!("{:?}", n.op)
    };
    assert!(items[0].1.sql().contains("upper"), "{}", items[0].1.sql());
    assert_eq!(items[1].1, Expr::null());
    assert!(n.notes.iter().any(|x| x.contains("does not assign B")));
    assert!(n.notes.iter().any(|x| x.contains("printing")));
}

#[test]
fn a_java_row_with_other_code_is_a_gap() {
    let body = [
        node(
            "tFileInputDelimited",
            "tFileInputDelimited_1",
            &format!("{}{}", param("FILENAME", "\"/in/a.csv\""), strings(&["A"])),
        ),
        node(
            "tJavaRow",
            "tJavaRow_1",
            &format!(
                "{}{}",
                param(
                    "CODE",
                    "for (int i = 0; i < 2; i++) { output_row.A = input_row.A; }"
                ),
                strings(&["A"])
            ),
        ),
        link("row1", "tFileInputDelimited_1", "tJavaRow_1"),
    ]
    .concat();
    let l = lowered(&body, "");
    assert!(of(&l.ir, "tJavaRow_1")[0].is_gap());
}

fn sequence_job(entries: &str) -> String {
    [
        node(
            "tFileInputDelimited",
            "tFileInputDelimited_1",
            &format!("{}{}", param("FILENAME", "\"/in/a.csv\""), strings(&["ID"])),
        ),
        node(
            "tSortRow",
            "tSortRow_1",
            &format!(
                "{}{}",
                table("CRITERIA", &[&[("COLNAME", "ID"), ("SORT", "alpha"), ("ORDER", "asc")]]),
                strings(&["ID"])
            ),
        ),
        node(
            "tMap",
            "tMap_1",
            &format!(
                r#"<nodeData xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="TalendMapper:MapperData"><outputTables name="out1">{entries}</outputTables><inputTables name="row2"/></nodeData>"#
            ),
        ),
        node("tLogRow", "tLogRow_1", &strings(&["ID", "N"])),
        link("row1", "tFileInputDelimited_1", "tSortRow_1"),
        link("row2", "tSortRow_1", "tMap_1"),
        link("out1", "tMap_1", "tLogRow_1"),
    ]
    .concat()
}

#[test]
fn a_sequence_numbers_the_rows_in_the_sorted_order() {
    let l = lowered(
        &sequence_job(
            r#"<mapperTableEntries name="ID" expression="row2.ID"/><mapperTableEntries name="N" expression="Numeric.sequence(&quot;s1&quot;, 1, 1)" type="id_Integer"/>"#,
        ),
        "",
    );
    no_gaps(&l);
    let map = of(&l.ir, "tMap_1");
    let order = map
        .iter()
        .find_map(|n| match &n.op {
            Op::Window { order, items, .. } => Some((order.clone(), items.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(order.0[0].field, "row2__ID");
    assert_eq!(order.1[0].0, "__seq_s1");
    let out = map.last().unwrap();
    assert!(out.notes.iter().any(|n| n.contains("tSortRow_1's order")));
}

#[test]
fn one_sequence_called_twice_is_a_gap() {
    let l = lowered(
        &sequence_job(
            r#"<mapperTableEntries name="ID" expression="Numeric.sequence(&quot;s1&quot;, 1, 1)"/><mapperTableEntries name="N" expression="Numeric.sequence(&quot;s1&quot;, 1, 1)"/>"#,
        ),
        "",
    );
    let map = of(&l.ir, "tMap_1");
    assert!(
        map.iter()
            .any(|n| matches!(&n.op, Op::Gap { reason } if reason.contains("called 2 times")))
    );
}

#[test]
fn a_sample_keeps_the_listed_lines_in_arrival_order() {
    let body = [
        node(
            "tFileInputDelimited",
            "tFileInputDelimited_1",
            &format!("{}{}", param("FILENAME", "\"/in/a.csv\""), strings(&["ID"])),
        ),
        node(
            "tSampleRow",
            "tSampleRow_1",
            &format!("{}{}", param("RANGE", "\"1,3..4\""), strings(&["ID"])),
        ),
        link("row1", "tFileInputDelimited_1", "tSampleRow_1"),
    ]
    .concat();
    let l = lowered(&body, "");
    no_gaps(&l);
    let f = of(&l.ir, "tSampleRow_1")
        .into_iter()
        .find_map(|n| match &n.op {
            Op::Filter { predicate } => Some(predicate.sql()),
            _ => None,
        })
        .unwrap();
    assert!(
        f.contains("= 1") && f.contains(">= 3") && f.contains("<= 4"),
        "{f}"
    );
}
