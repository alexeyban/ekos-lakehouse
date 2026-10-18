//! RFC 0202: whole transformations into the IR — a realistic fact flow and the graph rules.

use ekos_lakehouse_kettle::{SourceFile, read_package};
use ekos_lakehouse_kettle_expr::{Fidelity, KType};
use ekos_lakehouse_kettle_rel::{Catalog, Field, Ir, Op, SinkMode, TableRef, build};

struct Shop;

impl Catalog for Shop {
    fn columns(&self, t: &TableRef) -> Option<Vec<Field>> {
        let f = |n: &str, t| Field::new(n, t);
        match t.table.as_str() {
            "orders" => Some(vec![
                f("id", KType::Integer),
                f("customer_id", KType::Integer),
                f("ordered_at", KType::Timestamp),
                f("status", KType::String),
                f("qty", KType::Integer),
                f("price", KType::BigNumber),
            ]),
            _ => None,
        }
    }
}

const CONN: &str = "<connection><name>src</name><type>POSTGRESQL</type><database>shop</database>\
                    <attributes><attribute><code>PREFERRED_SCHEMA_NAME</code><attribute>public</attribute></attribute></attributes></connection>\
                    <connection><name>dwh</name><type>POSTGRESQL</type><database>dwh</database></connection>";

fn ktr(body: &str, hops: &[(&str, &str)]) -> String {
    let hops: String = hops
        .iter()
        .map(|(a, b)| format!("<hop><from>{a}</from><to>{b}</to><enabled>Y</enabled></hop>"))
        .collect();
    format!(
        "<transformation><info><name>t</name></info>{CONN}<order>{hops}</order>{body}</transformation>"
    )
}

fn ir_of(files: &[(&str, String)], path: &str) -> Ir {
    let files: Vec<SourceFile> = files
        .iter()
        .map(|(p, t)| SourceFile {
            path: p.to_string(),
            text: t.clone(),
        })
        .collect();
    let p = read_package(&files, &[]);
    build(&p, p.transformation(path).expect("read"), &Shop)
}

fn node<'a>(ir: &'a Ir, step: &str) -> &'a ekos_lakehouse_kettle_rel::RelNode {
    ir.nodes.iter().find(|n| n.step == step).unwrap_or_else(|| {
        panic!(
            "no node {step}: {:#?}",
            ir.nodes.iter().map(|n| &n.step).collect::<Vec<_>>()
        )
    })
}

#[test]
fn a_fact_flow_lowers_step_by_step() {
    let body = r#"
      <step><name>orders</name><type>TableInput</type><connection>src</connection>
        <sql>SELECT id, customer_id, ordered_at, status, qty, price FROM orders WHERE status &lt;&gt; 'void'</sql></step>
      <step><name>calc</name><type>Calculator</type>
        <calculation><field_name>amount</field_name><calc_type>MULTIPLY</calc_type><field_a>qty</field_a><field_b>price</field_b><value_type>BigNumber</value_type></calculation>
        <calculation><field_name>order_day</field_name><calc_type>REMOVE_TIME_FROM_DATE</calc_type><field_a>ordered_at</field_a><value_type>Date</value_type></calculation>
        <calculation><field_name>tmp</field_name><calc_type>CONSTANT</calc_type><field_a>1</field_a><value_type>Integer</value_type><remove>Y</remove></calculation>
      </step>
      <step><name>keep</name><type>FilterRows</type><send_true_to>sort</send_true_to><send_false_to>rejected</send_false_to>
        <compare><condition><leftvalue>amount</leftvalue><function>&gt;</function><value><type>Number</type><text>0</text></value></condition></compare></step>
      <step><name>rejected</name><type>Dummy</type></step>
      <step><name>sort</name><type>SortRows</type><fields><field><name>customer_id</name><ascending>Y</ascending><case_sensitive>Y</case_sensitive></field></fields></step>
      <step><name>per customer</name><type>GroupBy</type><group><field><name>customer_id</name></field></group>
        <fields><field><aggregate>total</aggregate><subject>amount</subject><type>SUM</type></field>
                <field><aggregate>orders</aggregate><subject>id</subject><type>COUNT_ALL</type></field></fields></step>
      <step><name>out</name><type>TableOutput</type><connection>dwh</connection><schema>marts</schema><table>customer_totals</table><truncate>Y</truncate></step>"#;
    let t = ktr(
        body,
        &[
            ("orders", "calc"),
            ("calc", "keep"),
            ("keep", "sort"),
            ("keep", "rejected"),
            ("sort", "per customer"),
            ("per customer", "out"),
        ],
    );
    let ir = ir_of(&[("t.ktr", t)], "t.ktr");
    let src = node(&ir, "orders");
    assert!(matches!(&src.op, Op::Source { .. }));
    assert_eq!(src.schema.len(), 6);
    let calc = node(&ir, "calc");
    let names: Vec<_> = calc.schema.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "id",
            "customer_id",
            "ordered_at",
            "status",
            "qty",
            "price",
            "amount",
            "order_day"
        ],
        "removed field dropped"
    );
    let Op::Project { items } = &calc.op else {
        panic!()
    };
    assert_eq!(items[6].1.sql(), "(\"qty\" * \"price\")");
    let routed: Vec<_> = ir
        .nodes
        .iter()
        .filter(|n| n.step == "keep")
        .map(|n| n.branch.clone())
        .collect();
    assert_eq!(
        routed,
        vec![Some("sort".to_string()), Some("rejected".to_string())]
    );
    let agg = node(&ir, "per customer");
    assert_eq!(agg.fidelity, Fidelity::Exact, "{:?}", agg.notes);
    assert_eq!(
        agg.schema
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        vec!["customer_id", "total", "orders"]
    );
    let out = node(&ir, "out");
    let Op::Sink { sink } = &out.op else { panic!() };
    assert_eq!(
        (
            sink.mode,
            sink.target.table.as_str(),
            sink.target.schema.as_deref()
        ),
        (SinkMode::Insert, "customer_totals", Some("marts"))
    );
    assert!(
        ir.nodes.iter().all(|n| n.fidelity != Fidelity::Gap),
        "{:#?}",
        ir.nodes.iter().filter(|n| n.is_gap()).collect::<Vec<_>>()
    );
}

#[test]
fn group_by_on_unsorted_input_is_rewritten_not_exact() {
    let body = r#"
      <step><name>orders</name><type>TableInput</type><connection>src</connection><sql>select * from orders</sql></step>
      <step><name>g</name><type>GroupBy</type><group><field><name>status</name></field></group>
        <fields><field><aggregate>n</aggregate><subject>id</subject><type>COUNT_ALL</type></field></fields></step>"#;
    let ir = ir_of(&[("t.ktr", ktr(body, &[("orders", "g")]))], "t.ktr");
    let g = node(&ir, "g");
    assert_eq!(g.fidelity, Fidelity::Rewritten);
    assert!(
        g.notes.iter().any(|n| n.contains("consecutive")),
        "{:?}",
        g.notes
    );
}

#[test]
fn distribution_error_hops_and_unknown_steps_become_gaps() {
    let body = r#"
      <step><name>orders</name><type>TableInput</type><connection>src</connection><sql>select * from orders</sql><distribute>Y</distribute></step>
      <step><name>a</name><type>Dummy</type></step>
      <step><name>b</name><type>Dummy</type></step>
      <step><name>plugin</name><type>com.acme.Magic</type></step>
      <step><name>after</name><type>Dummy</type></step>"#;
    let t = ktr(
        body,
        &[
            ("orders", "a"),
            ("orders", "b"),
            ("a", "plugin"),
            ("plugin", "after"),
        ],
    );
    let ir = ir_of(&[("t.ktr", t)], "t.ktr");
    let a = node(&ir, "a");
    assert!(
        matches!(&a.op, Op::Gap { reason } if reason.contains("round-robin")),
        "{:?}",
        a.op
    );
    let after = node(&ir, "after");
    assert!(after.is_gap(), "a gap propagates downstream");
    // An error hop: the step is rewritten and the error target is a gap.
    let body = r#"
      <step><name>orders</name><type>TableInput</type><connection>src</connection><sql>select * from orders</sql></step>
      <step><name>calc</name><type>Calculator</type>
        <calculation><field_name>x</field_name><calc_type>ABS</calc_type><field_a>qty</field_a><value_type>Integer</value_type></calculation></step>
      <step><name>bad</name><type>Dummy</type></step>
      <step_error_handling><error><source_step>calc</source_step><target_step>bad</target_step><is_enabled>Y</is_enabled></error></step_error_handling>"#;
    let ir = ir_of(
        &[("t.ktr", ktr(body, &[("orders", "calc"), ("calc", "bad")]))],
        "t.ktr",
    );
    assert_eq!(node(&ir, "calc").fidelity, Fidelity::Rewritten);
    assert!(node(&ir, "bad").is_gap());
}

#[test]
fn a_mapping_is_inlined_with_its_renames() {
    let child = r#"<transformation><info><name>child</name></info>
      <order><hop><from>in</from><to>calc</to><enabled>Y</enabled></hop><hop><from>calc</from><to>out</to><enabled>Y</enabled></hop></order>
      <step><name>in</name><type>MappingInput</type></step>
      <step><name>calc</name><type>Calculator</type>
        <calculation><field_name>res</field_name><calc_type>ADD</calc_type><field_a>leftValue</field_a><field_b>rightValue</field_b><value_type>Integer</value_type></calculation></step>
      <step><name>out</name><type>MappingOutput</type></step></transformation>"#;
    let parent_body = r#"
      <step><name>orders</name><type>TableInput</type><connection>src</connection><sql>select id, qty from orders</sql></step>
      <step><name>m</name><type>Mapping</type><specification_method>filename</specification_method>
        <filename>${Internal.Transformation.Filename.Directory}/child.ktr</filename>
        <mappings><input><mapping><main_path>Y</main_path><rename_on_output>Y</rename_on_output>
          <connector><parent>id</parent><child>leftValue</child></connector>
          <connector><parent>qty</parent><child>rightValue</child></connector></mapping></input>
          <output><mapping><connector><parent>res</parent><child>total</child></connector></mapping></output></mappings></step>"#;
    let parent = ktr(parent_body, &[("orders", "m")]);
    let ir = ir_of(
        &[
            ("etl/parent.ktr", parent),
            ("etl/child.ktr", child.to_string()),
        ],
        "etl/parent.ktr",
    );
    let m = ir.nodes.iter().rev().find(|n| n.step == "m").unwrap();
    assert_eq!(
        m.schema.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        vec!["id", "qty", "total"]
    );
    assert!(
        ir.nodes.iter().any(|n| n.step == "m/calc"),
        "child steps are prefixed"
    );
    assert!(
        ir.nodes.iter().all(|n| !n.is_gap()),
        "{:#?}",
        ir.nodes.iter().filter(|n| n.is_gap()).collect::<Vec<_>>()
    );
}

#[test]
fn switch_case_routes_and_lookups_join() {
    let body = r#"
      <step><name>orders</name><type>TableInput</type><connection>src</connection><sql>select * from orders</sql></step>
      <step><name>status names</name><type>DataGrid</type>
        <fields><field><name>code</name><type>String</type></field><field><name>label</name><type>String</type></field></fields>
        <data><line><item>new</item><item>New order</item></line><line><item>paid</item><item>Paid</item></line></data></step>
      <step><name>label</name><type>StreamLookup</type><from>status names</from>
        <lookup><key><name>status</name><field>code</field></key><value><name>label</name><rename>status_label</rename><default>unknown</default><type>String</type></value></lookup></step>
      <step><name>route</name><type>SwitchCase</type><fieldname>status</fieldname><case_value_type>String</case_value_type><default_target_step>other</default_target_step>
        <cases><case><value>new</value><target_step>fresh</target_step></case><case><value>paid</value><target_step>fresh</target_step></case></cases></step>
      <step><name>fresh</name><type>Dummy</type></step>
      <step><name>other</name><type>Dummy</type></step>"#;
    let t = ktr(
        body,
        &[
            ("orders", "label"),
            ("status names", "label"),
            ("label", "route"),
            ("route", "fresh"),
            ("route", "other"),
        ],
    );
    let ir = ir_of(&[("t.ktr", t)], "t.ktr");
    let l = node(&ir, "label");
    assert!(
        matches!(&l.op, Op::Lookup { keys, .. } if keys == &[("status".to_string(), "code".to_string())])
    );
    assert_eq!(l.inputs.len(), 2);
    let fresh = ir
        .nodes
        .iter()
        .find(|n| n.step == "route" && n.branch.as_deref() == Some("fresh"))
        .unwrap();
    let Op::Filter { predicate } = &fresh.op else {
        panic!()
    };
    assert!(
        predicate.sql().contains(" OR "),
        "two cases to one target are OR-ed: {}",
        predicate.sql()
    );
    assert!(!node(&ir, "other").is_gap());
}
