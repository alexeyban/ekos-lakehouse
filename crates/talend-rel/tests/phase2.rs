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

/// EKOS RFC 0268 phase 2: fixture B's calendar, a `tRowGenerator` whose `NB_ROWS` a `tJava` sets.
fn calendar_job(code: &str, week: bool) -> String {
    let expr = if week {
        "TalendDate.getPartOfDate(\"WEEK_OF_YEAR\", TalendDate.addDate(context.dtDeb, Numeric.sequence(\"s1\", 0, 1), \"dd\"))"
    } else {
        "TalendDate.addDate(context.dtDeb, Numeric.sequence(\"s1\", 0, 1), \"dd\")"
    };
    [
        node("tJava", "tJava_3", &param("CODE", code)),
        node(
            "tRowGenerator",
            "tRowGenerator_1",
            &format!(
                "{}{}{}",
                param("NB_ROWS", "context.nbJours"),
                table("VALUES", &[&[("SCHEMA_COLUMN", "D"), ("ARRAY", expr)]]),
                meta(&[("D", if week { "id_Integer" } else { "id_Date" })])
            ),
        ),
        node("tLogRow", "tLogRow_1", &meta(&[("D", "id_Date")])),
        link("row1", "tRowGenerator_1", "tLogRow_1"),
    ]
    .concat()
}

const CALENDAR_CONTEXT: &str = r#"<contextParameter name="dtDeb" type="id_Date" value="2020-01-01 00:00:00"/><contextParameter name="dtFin" type="id_Date" value="2025-12-31 00:00:00"/><contextParameter name="nbJours" type="id_Integer" value="0"/>"#;

#[test]
fn a_row_count_a_tjava_sets_from_two_dates_is_computed_from_their_defaults() {
    let l = lowered(
        &calendar_job(
            "context.nbJours = (int)TalendDate.diffDate(context.dtFin, context.dtDeb, \"dd\")+1;\nSystem.out.println(\"days: \" + context.nbJours);",
            false,
        ),
        CALENDAR_CONTEXT,
    );
    no_gaps(&l);
    let g = of(&l.ir, "tRowGenerator_1")
        .into_iter()
        .find(|n| matches!(n.op, Op::Generate { .. }))
        .unwrap();
    // 2020-01-01 … 2025-12-31: 2192 days (two leap years).
    assert!(
        matches!(g.op, Op::Generate { count: 2192, .. }),
        "{:?}",
        g.op
    );
    assert_eq!(g.fidelity, Fidelity::Rewritten);
    assert!(
        g.notes
            .iter()
            .any(|n| n.contains("set at run time by tJava_3"))
    );
}

#[test]
fn a_row_count_a_tjava_sets_in_another_form_is_a_gap_not_its_default() {
    let l = lowered(
        &calendar_job("context.nbJours = Integer.parseInt(context.dtFin);", false),
        CALENDAR_CONTEXT,
    );
    let gap =
        l.ir.nodes
            .iter()
            .find_map(|n| match &n.op {
                Op::Gap { reason } => Some(reason.clone()),
                _ => None,
            })
            .expect("a gap, not the default count 0");
    assert!(gap.contains("set at run time by tJava_3"), "{gap}");
}

#[test]
fn week_of_year_is_a_gap_unless_the_locale_is_stated() {
    let code =
        "context.nbJours = (int)TalendDate.diffDate(context.dtFin, context.dtDeb, \"dd\")+1;";
    let l = lowered(&calendar_job(code, true), CALENDAR_CONTEXT);
    assert!(
        l.ir.nodes
            .iter()
            .any(|n| matches!(&n.op, Op::Gap { reason } if reason.contains("--locale")))
    );

    let xml = format!(
        r#"<talendfile:ProcessType xmlns:talendfile="platform:/resource/org.talend.model/model/TalendFile.xsd" defaultContext="Default"><context name="Default">{CALENDAR_CONTEXT}</context>{}</talendfile:ProcessType>"#,
        calendar_job(code, true)
    );
    let p = read_project(
        "",
        &[ProjectFile {
            path: "process/j_0.1.item".into(),
            text: xml,
        }],
    );
    let job = p.expand(&p.jobs[0]).job;
    let l = ekos_lakehouse_talend_rel::lower_job_with(
        &job,
        &p.routines,
        &NoCatalog,
        &ekos_lakehouse_talend_rel::Options {
            locale: ekos_lakehouse_talend_rel::Locale::parse("fr_FR"),
            ..Default::default()
        },
    );
    no_gaps(&l);
    let out = of(&l.ir, "tRowGenerator_1").into_iter().last().unwrap();
    assert!(format!("{:?}", out.op).contains("IsoWeek"), "{:?}", out.op);
    assert!(out.notes.iter().any(|n| n.contains("ISO week")));
}

#[test]
fn the_file_a_row_came_from_travels_through_a_tmap_to_a_later_one() {
    let map = |name: &str, input: &str, entries: &str| {
        node(
            "tMap",
            name,
            &format!(
                r#"<nodeData xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="TalendMapper:MapperData"><outputTables name="{name}_out">{entries}</outputTables><inputTables name="{input}"/></nodeData>"#
            ),
        )
    };
    let body = [
        node("tFileList", "tFileList_1", &param("DIRECTORY", "\"/in\"")),
        node(
            "tFileInputDelimited",
            "tFileInputDelimited_1",
            &format!(
                "{}{}",
                param(
                    "FILENAME",
                    "((String)globalMap.get(\"tFileList_1_CURRENT_FILEPATH\"))"
                ),
                strings(&["ID"])
            ),
        ),
        map(
            "tMap_1",
            "row1",
            r#"<mapperTableEntries name="ID" expression="row1.ID"/>"#,
        ),
        map(
            "tMap_2",
            "tMap_1_out",
            r#"<mapperTableEntries name="F" expression="((String)globalMap.get(&quot;tFileList_1_CURRENT_FILEPATH&quot;))"/>"#,
        ),
        node("tLogRow", "tLogRow_1", &strings(&["F"])),
        link("row1", "tFileInputDelimited_1", "tMap_1"),
        link("tMap_1_out", "tMap_1", "tMap_2"),
        link("tMap_2_out", "tMap_2", "tLogRow_1"),
    ]
    .concat();
    let l = lowered(&body, "");
    no_gaps(&l);
    let out = of(&l.ir, "tMap_2").into_iter().last().unwrap();
    let Op::Project { items } = &out.op else {
        panic!("{:?}", out.op)
    };
    let f = &items.iter().find(|(n, _)| n == "F").unwrap().1;
    // Read from the second tMap's own input, which the first one carried it to.
    assert_eq!(f.columns(), vec!["tMap_1_out__$path".to_string()], "{f:?}");
    let input = l.ir.node(out.inputs[0]);
    assert!(
        l.ir.nodes
            .iter()
            .any(|n| n.schema.iter().any(|x| x.name == "tMap_1_out__$path")),
        "{:?}",
        input.schema
    );
}

#[test]
fn a_month_name_follows_the_locale_the_operator_states() {
    let body = [
        node(
            "tRowGenerator",
            "tRowGenerator_1",
            &format!(
                "{}{}{}",
                param("NB_ROWS", "1"),
                table(
                    "VALUES",
                    &[&[
                        ("SCHEMA_COLUMN", "M"),
                        ("ARRAY", "TalendDate.formatDate(\"MMMM\", TalendDate.parseDate(\"yyyy-MM-dd\", \"2020-08-01\"))"),
                    ]]
                ),
                strings(&["M"])
            ),
        ),
        node("tLogRow", "tLogRow_1", &strings(&["M"])),
        link("row1", "tRowGenerator_1", "tLogRow_1"),
    ]
    .concat();
    let l = lowered(&body, "");
    assert!(
        l.ir.nodes.iter().any(|n| n.is_gap()),
        "a gap without a locale"
    );
    let xml = format!(
        r#"<talendfile:ProcessType xmlns:talendfile="platform:/resource/org.talend.model/model/TalendFile.xsd" defaultContext="Default"><context name="Default"></context>{body}</talendfile:ProcessType>"#
    );
    let p = read_project(
        "",
        &[ProjectFile {
            path: "process/j_0.1.item".into(),
            text: xml,
        }],
    );
    let job = p.expand(&p.jobs[0]).job;
    let l = ekos_lakehouse_talend_rel::lower_job_with(
        &job,
        &p.routines,
        &NoCatalog,
        &ekos_lakehouse_talend_rel::Options {
            locale: ekos_lakehouse_talend_rel::Locale::parse("fr-FR"),
            ..Default::default()
        },
    );
    no_gaps(&l);
    let g = of(&l.ir, "tRowGenerator_1").into_iter().last().unwrap();
    let text = format!("{:?}", g.op);
    assert!(
        text.contains("\"août\"") && text.contains("\"janvier\""),
        "{text}"
    );
    assert!(g.notes.iter().any(|n| n.contains("fr_FR")));
    // A US week is not the ISO week: still a gap.
    assert!(
        !ekos_lakehouse_talend_rel::Locale::parse("en_US")
            .unwrap()
            .iso_weeks
    );
    assert!(ekos_lakehouse_talend_rel::Locale::parse("xx_YY").is_none());
}

#[test]
fn an_empty_schema_on_a_component_falls_back_to_its_connections() {
    let body = [
        node(
            "tPostgresqlConnection",
            "tDBConnection_1",
            &format!(
                "{}{}",
                param("SCHEMA_DB", "context.schema_dwh"),
                param("DBNAME", "\"db\"")
            ),
        ),
        node(
            "tRowGenerator",
            "tRowGenerator_1",
            &format!(
                "{}{}{}",
                param("NB_ROWS", "1"),
                table("VALUES", &[&[("SCHEMA_COLUMN", "A"), ("ARRAY", "\"x\"")]]),
                strings(&["A"])
            ),
        ),
        node(
            "tPostgresqlOutput",
            "tDBOutput_1",
            &format!(
                "{}{}{}{}{}{}",
                param("TABLE", "\"DIM_CALENDRIER\""),
                param("SCHEMA_DB", "\"\""),
                param("DBNAME", "\"\""),
                param("USE_EXISTING_CONNECTION", "true"),
                param("CONNECTION", "tDBConnection_1"),
                strings(&["A"])
            ),
        ),
        link("row1", "tRowGenerator_1", "tDBOutput_1"),
    ]
    .concat();
    let l = lowered(
        &body,
        r#"<contextParameter name="schema_dwh" type="id_String" value="VENTE_DWH"/>"#,
    );
    let sink =
        l.ir.nodes
            .iter()
            .find_map(|n| match &n.op {
                Op::Sink { sink } => Some(sink.clone()),
                _ => None,
            })
            .unwrap();
    assert_eq!(sink.target.schema.as_deref(), Some("VENTE_DWH"));
    assert_eq!(
        sink.target.connection.unwrap().database.as_deref(),
        Some("db")
    );
}

/// RFC 0268 phase 3: a gap with one known input is offered for a model's draft; a checked draft
/// replaces it, labelled `llm-drafted`.
#[test]
fn a_gap_is_offered_for_a_draft_and_a_draft_replaces_it() {
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
    let [d] = l.draftable.as_slice() else {
        panic!("{:?}", l.draftable)
    };
    assert_eq!((d.component.as_str(), d.port.as_str()), ("tJavaRow_1", ""));
    assert!(d.original.contains("output_row.A"));
    assert_eq!(d.outputs.len(), 1);

    let xml = format!(
        r#"<talendfile:ProcessType xmlns:talendfile="platform:/resource/org.talend.model/model/TalendFile.xsd" defaultContext="Default"><context name="Default"></context>{body}</talendfile:ProcessType>"#
    );
    let p = read_project(
        "",
        &[ProjectFile {
            path: "process/j_0.1.item".into(),
            text: xml,
        }],
    );
    let job = p.expand(&p.jobs[0]).job;
    let draft = ekos_lakehouse_etl_rel::Draft {
        columns: vec![("A".into(), "\"A\"".into())],
        keep_when: Some("\"A\" IS NOT NULL".into()),
        model: "a local model".into(),
    };
    let l = ekos_lakehouse_talend_rel::lower_job_with(
        &job,
        &p.routines,
        &NoCatalog,
        &ekos_lakehouse_talend_rel::Options {
            drafts: [(
                (job.name.clone(), "tJavaRow_1".to_string(), String::new()),
                draft,
            )]
            .into(),
            ..Default::default()
        },
    );
    no_gaps(&l);
    let nodes = of(&l.ir, "tJavaRow_1");
    assert!(nodes.iter().all(|n| n.fidelity == Fidelity::LlmDrafted));
    assert!(matches!(nodes[0].op, Op::Filter { .. }));
    assert!(matches!(nodes.last().unwrap().op, Op::Project { .. }));
    assert!(l.draftable.is_empty());
}
