//! 结构化输出协议解析（纯函数，便于单测）
//!
//! 脚本在标准输出中以 `modelinfer:` 开头的行夹杂结构化数据（与 `cargo build.rs` 的 `cargo:`
//! 约定同构）。`modelinfer:` 之后的内容为 `DATA`，支持两类：
//!
//! 1. **JSON**：JSON Patch 数组 / 含 `op` 的单操作对象 / 普通对象合并；
//! 2. **简化 `KEY op VALUE`**：`=`（字符串）、`:=`（JSON）、`+=`（数组追加）、`-=`（数组移除）。
//!
//! 一切「尽力而为」：非法 / 未识别内容一律忽略，永不 panic、永不影响阶段成败。

use serde_json::{Map, Value};

/// 结构化输出前缀
pub const PREFIX: &str = "modelinfer:";

/// 单个命令中单条 `modelinfer:` 行解析出的指令
#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    /// JSON Patch 操作列表（作用于节点对象，`/global` 前缀路由到全局）
    Patch(Vec<PatchOp>),
    /// 合并进节点对象
    Merge(Value),
    /// 合并进阶段全局对象
    GlobalMerge(Value),
    /// `KEY=VALUE`（字符串）
    Set { path: Vec<PathSeg>, value: Value },
    /// `KEY:=VALUE`（JSON，失败回退字符串）
    SetJson { path: Vec<PathSeg>, value: Value },
    /// `KEY+=VALUE`（数组追加）
    Append { path: Vec<PathSeg>, value: Value },
    /// `KEY-=VALUE`（数组移除）
    RemoveValue { path: Vec<PathSeg>, value: Value },
}

/// JSON Pointer 路径段
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    Key(String),
    Index(usize),
}

/// JSON Patch 操作
#[derive(Debug, Clone, PartialEq)]
pub struct PatchOp {
    pub kind: PatchOpKind,
    pub path: Vec<PathSeg>,
    pub value: Option<Value>,
}

/// JSON Patch 操作类型（仅实现子集，未知 op 忽略）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOpKind {
    Add,
    Replace,
    Remove,
}

/// 单节点聚合结果
#[derive(Debug, Clone, PartialEq)]
pub struct NodeAggregate {
    /// 节点对象
    pub node: Value,
    /// 待合并进阶段全局对象的贡献（对象）
    pub global_patch: Value,
}

impl Default for NodeAggregate {
    fn default() -> Self {
        Self {
            node: Value::Object(Map::new()),
            global_patch: Value::Object(Map::new()),
        }
    }
}

/// 从完整标准输出中提取全部结构化指令（忽略非法行）
pub fn extract_directives(stdout: &str) -> Vec<Directive> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        let Some(data) = trimmed.strip_prefix(PREFIX) else {
            continue;
        };
        out.extend(parse_data(data));
    }
    out
}

/// 解析一行 `DATA`（前缀之后的内容）
pub fn parse_data(data: &str) -> Vec<Directive> {
    let data = data.trim();
    if data.is_empty() {
        return Vec::new();
    }
    if let Ok(v) = serde_json::from_str::<Value>(data) {
        return from_json(v);
    }
    parse_simplified(data).into_iter().collect()
}

/// JSON 通道解析
fn from_json(v: Value) -> Vec<Directive> {
    match v {
        Value::Array(items) => {
            let ops: Vec<PatchOp> = items.iter().filter_map(parse_patch_op).collect();
            if ops.is_empty() {
                Vec::new()
            } else {
                vec![Directive::Patch(ops)]
            }
        }
        Value::Object(map) => {
            // 含 op 字段 → 单操作
            if map.contains_key("op") {
                return parse_patch_op(&Value::Object(map))
                    .map(|op| vec![Directive::Patch(vec![op])])
                    .unwrap_or_default();
            }
            let mut out = Vec::new();
            // 拆分 global（值为对象时）
            if let Some(Value::Object(global)) = map.get("global") {
                let mut global_map = Map::new();
                for (k, v) in global {
                    global_map.insert(k.clone(), v.clone());
                }
                out.push(Directive::GlobalMerge(Value::Object(global_map)));
            }
            // 其余键合并进节点对象
            let mut rest = Map::new();
            for (k, v) in &map {
                if k == "global" {
                    continue;
                }
                rest.insert(k.clone(), v.clone());
            }
            if !rest.is_empty() {
                out.push(Directive::Merge(Value::Object(rest)));
            }
            out
        }
        _ => Vec::new(),
    }
}

/// 解析单个 JSON Patch 操作（未知 op / 无 path 返回 None）
fn parse_patch_op(v: &Value) -> Option<PatchOp> {
    let obj = v.as_object()?;
    let kind = match obj.get("op").and_then(Value::as_str) {
        Some("add") => PatchOpKind::Add,
        Some("replace") => PatchOpKind::Replace,
        Some("remove") => PatchOpKind::Remove,
        _ => return None,
    };
    let path = parse_json_pointer(obj.get("path").and_then(Value::as_str).unwrap_or(""))?;
    Some(PatchOp {
        kind,
        path,
        value: obj.get("value").cloned(),
    })
}

/// 解析 JSON Pointer（`/a/b/0`）；空指针合法（根）
pub fn parse_json_pointer(raw: &str) -> Option<Vec<PathSeg>> {
    if raw.is_empty() {
        return Some(Vec::new());
    }
    if !raw.starts_with('/') {
        return None;
    }
    let mut out = Vec::new();
    for seg in raw.split('/').skip(1) {
        let unescaped = seg.replace("~1", "/").replace("~0", "~");
        out.push(segment_of(&unescaped));
    }
    Some(out)
}

/// 简化语法解析：`KEY op VALUE`
pub fn parse_simplified(data: &str) -> Option<Directive> {
    // 最左优先；同一位置先匹配双字符操作符
    let bytes = data.as_bytes();
    let mut idx = None;
    let mut op = "";
    let mut i = 0;
    while i < bytes.len() {
        let rest = &data[i..];
        if rest.starts_with(":=") {
            idx = Some(i);
            op = ":=";
            break;
        }
        if rest.starts_with("+=") {
            idx = Some(i);
            op = "+=";
            break;
        }
        if rest.starts_with("-=") {
            idx = Some(i);
            op = "-=";
            break;
        }
        if rest.starts_with('=') {
            idx = Some(i);
            op = "=";
            break;
        }
        i += 1;
    }
    let idx = idx?;
    let key = data[..idx].trim();
    let path = parse_dotted_path(key)?;
    let value_raw = data[idx + op.len()..].trim();
    let value = match op {
        "=" => Value::String(value_raw.to_string()),
        ":=" => parse_json_or_string(value_raw),
        "+=" => parse_json_or_string(value_raw),
        "-=" => parse_json_or_string(value_raw),
        _ => return None,
    };
    Some(match op {
        "=" => Directive::Set { path, value },
        ":=" => Directive::SetJson { path, value },
        "+=" => Directive::Append { path, value },
        "-=" => Directive::RemoveValue { path, value },
        _ => return None,
    })
}

/// 点分路径解析（数字段视为数组索引）
pub fn parse_dotted_path(raw: &str) -> Option<Vec<PathSeg>> {
    if raw.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for seg in raw.split('.') {
        if seg.is_empty() {
            return None;
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        out.push(segment_of(seg));
    }
    Some(out)
}

fn segment_of(seg: &str) -> PathSeg {
    if !seg.is_empty()
        && seg.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n) = seg.parse::<usize>()
    {
        return PathSeg::Index(n);
    }
    PathSeg::Key(seg.to_string())
}

fn parse_json_or_string(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// 应用指令序列，得到节点对象与全局贡献
pub fn aggregate(directives: &[Directive]) -> NodeAggregate {
    let mut result = NodeAggregate::default();
    for d in directives {
        apply_directive(&mut result, d);
    }
    result
}

fn apply_directive(result: &mut NodeAggregate, d: &Directive) {
    match d {
        Directive::Patch(ops) => {
            for op in ops {
                if let Some(rest) = global_tail(&op.path) {
                    let op2 = PatchOp {
                        kind: op.kind,
                        path: rest,
                        value: op.value.clone(),
                    };
                    apply_patch(&mut result.global_patch, &op2);
                } else {
                    apply_patch(&mut result.node, op);
                }
            }
        }
        Directive::Merge(v) => deep_merge(&mut result.node, v),
        Directive::GlobalMerge(v) => deep_merge(&mut result.global_patch, v),
        Directive::Set { path, value } | Directive::SetJson { path, value } => {
            set_path(&mut result.node, path, value.clone());
        }
        Directive::Append { path, value } => {
            append_path(&mut result.node, path, value.clone());
        }
        Directive::RemoveValue { path, value } => {
            remove_value_path(&mut result.node, path, value);
        }
    }
}

/// 若路径以 `global` 开头，返回去掉该段后的路径
fn global_tail(path: &[PathSeg]) -> Option<Vec<PathSeg>> {
    match path.first() {
        Some(PathSeg::Key(k)) if k == "global" => Some(path[1..].to_vec()),
        _ => None,
    }
}

/// 深合并：对象递归合并、数组 / 标量替换、`null` 删除键
pub fn deep_merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    let slot = t.entry(k.clone()).or_insert(Value::Null);
                    if slot.is_object() && v.is_object() {
                        deep_merge(slot, v);
                    } else {
                        *slot = v.clone();
                    }
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

/// 应用 JSON Patch 操作（子集：add / replace / remove）
pub fn apply_patch(root: &mut Value, op: &PatchOp) {
    match op.kind {
        PatchOpKind::Add => set_path_append(root, &op.path, op.value.clone().unwrap_or(Value::Null)),
        PatchOpKind::Replace => {
            if op.value.is_some() {
                set_path(root, &op.path, op.value.clone().unwrap());
            }
        }
        PatchOpKind::Remove => remove_path(root, &op.path),
    }
}

/// 写路径（普通赋值；缺失的中间容器自动创建）
pub fn set_path(root: &mut Value, path: &[PathSeg], value: Value) {
    if path.is_empty() {
        *root = value;
        return;
    }
    let mut cur = root;
    for (i, seg) in path.iter().enumerate() {
        if i == path.len() - 1 {
            assign(cur, seg, value);
            return;
        }
        let next = &path[i + 1];
        cur = descend(cur, seg, Some(next));
    }
}

/// 追加写路径：`-` 或越界索引视为数组追加
pub fn set_path_append(root: &mut Value, path: &[PathSeg], value: Value) {
    if path.is_empty() {
        *root = value;
        return;
    }
    let mut cur = root;
    for (i, seg) in path.iter().enumerate() {
        if i == path.len() - 1 {
            append_assign(cur, seg, value);
            return;
        }
        let next = &path[i + 1];
        cur = descend(cur, seg, Some(next));
    }
}

/// 删除路径（不存在则忽略）
pub fn remove_path(root: &mut Value, path: &[PathSeg]) {
    if path.is_empty() {
        *root = Value::Null;
        return;
    }
    let Some(parent) = get_path_mut(root, &path[..path.len() - 1]) else {
        return;
    };
    match (&path[path.len() - 1], parent) {
        (PathSeg::Key(k), Value::Object(map)) => {
            map.remove(k);
        }
        (PathSeg::Index(i), Value::Array(arr)) if *i < arr.len() => {
            arr.remove(*i);
        }
        _ => {}
    }
}

/// `-=`：从数组移除等于 `value` 的元素（JSON 值相等）
fn remove_value_path(root: &mut Value, path: &[PathSeg], value: &Value) {
    let Some(target) = get_path_mut(root, path) else {
        return;
    };
    if let Value::Array(arr) = target {
        arr.retain(|v| v != value);
    }
}

/// `+=`：向 `path` 指向的数组追加元素（目标非数组时先包装为数组）
fn append_path(root: &mut Value, path: &[PathSeg], value: Value) {
    if path.is_empty() {
        return;
    }
    let (parent_path, last) = path.split_at(path.len() - 1);
    let last = &last[0];
    let Some(parent) = get_or_create_path(root, parent_path) else {
        return;
    };
    match (last, parent) {
        (PathSeg::Key(k), Value::Object(map)) => {
            let slot = map.entry(k.clone()).or_insert(Value::Array(Vec::new()));
            if !slot.is_array() {
                let old = slot.clone();
                *slot = Value::Array(vec![old]);
            }
            if let Some(arr) = slot.as_array_mut() {
                arr.push(value);
            }
        }
        (PathSeg::Index(i), Value::Array(arr)) => {
            while arr.len() <= *i {
                arr.push(Value::Null);
            }
            let slot = &mut arr[*i];
            if !slot.is_array() {
                let old = slot.clone();
                *slot = Value::Array(vec![old]);
            }
            if let Some(a) = slot.as_array_mut() {
                a.push(value);
            }
        }
        _ => {}
    }
}

/// 取 / 创建路径指向的容器（缺失的中间容器按下一段类型自动创建）
fn get_or_create_path<'a>(root: &'a mut Value, path: &[PathSeg]) -> Option<&'a mut Value> {
    let mut cur = root;
    for (i, seg) in path.iter().enumerate() {
        let next = path.get(i + 1);
        cur = descend(cur, seg, next);
    }
    Some(cur)
}

/// 下一段的容器类型：数组索引或 `-` 追加标记 → 数组，否则对象
fn container_for(next: &PathSeg) -> Value {
    match next {
        PathSeg::Index(_) => Value::Array(Vec::new()),
        PathSeg::Key(k) if k == "-" => Value::Array(Vec::new()),
        PathSeg::Key(_) => Value::Object(Map::new()),
    }
}

fn descend<'a>(cur: &'a mut Value, seg: &PathSeg, next: Option<&PathSeg>) -> &'a mut Value {
    match seg {
        PathSeg::Key(k) => {
            if !cur.is_object() {
                *cur = Value::Object(Map::new());
            }
            let map = cur.as_object_mut().expect("just ensured object");
            let slot = map.entry(k.clone()).or_insert(Value::Null);
            match next {
                Some(next) => {
                    let want = container_for(next);
                    let compatible = matches!(
                        (&want, &*slot),
                        (Value::Array(_), Value::Array(_)) | (Value::Object(_), Value::Object(_))
                    );
                    if !compatible {
                        *slot = want;
                    }
                }
                None => {
                    if slot.is_null() {
                        *slot = Value::Object(Map::new());
                    }
                }
            }
            slot
        }
        PathSeg::Index(i) => {
            if !cur.is_array() {
                *cur = Value::Array(Vec::new());
            }
            let arr = cur.as_array_mut().expect("just ensured array");
            while arr.len() <= *i {
                arr.push(Value::Null);
            }
            let slot = &mut arr[*i];
            match next {
                Some(next) => {
                    let want = container_for(next);
                    let compatible = matches!(
                        (&want, &*slot),
                        (Value::Array(_), Value::Array(_)) | (Value::Object(_), Value::Object(_))
                    );
                    if !compatible {
                        *slot = want;
                    }
                }
                None => {
                    if slot.is_null() {
                        *slot = Value::Object(Map::new());
                    }
                }
            }
            slot
        }
    }
}

fn assign(cur: &mut Value, seg: &PathSeg, value: Value) {
    match seg {
        PathSeg::Key(k) => {
            if !cur.is_object() {
                *cur = Value::Object(Map::new());
            }
            if let Some(map) = cur.as_object_mut() {
                map.insert(k.clone(), value);
            }
        }
        PathSeg::Index(i) => {
            if !cur.is_array() {
                *cur = Value::Array(Vec::new());
            }
            if let Some(arr) = cur.as_array_mut() {
                while arr.len() <= *i {
                    arr.push(Value::Null);
                }
                arr[*i] = value;
            }
        }
    }
}

fn append_assign(cur: &mut Value, seg: &PathSeg, value: Value) {
    match seg {
        PathSeg::Key(k) if k == "-" => {
            if !cur.is_array() {
                *cur = Value::Array(Vec::new());
            }
            if let Some(arr) = cur.as_array_mut() {
                arr.push(value);
            }
        }
        PathSeg::Index(i) => {
            if !cur.is_array() {
                *cur = Value::Array(Vec::new());
            }
            if let Some(arr) = cur.as_array_mut() {
                if *i >= arr.len() {
                    arr.push(value);
                } else {
                    arr[*i] = value;
                }
            }
        }
        _ => assign(cur, seg, value),
    }
}

fn get_path_mut<'a>(root: &'a mut Value, path: &[PathSeg]) -> Option<&'a mut Value> {
    let mut cur = root;
    for seg in path {
        match (seg, cur) {
            (PathSeg::Key(k), Value::Object(map)) => cur = map.get_mut(k)?,
            (PathSeg::Index(i), Value::Array(arr)) => cur = arr.get_mut(*i)?,
            _ => return None,
        }
    }
    Some(cur)
}

/// 从节点对象提取容器清单（字符串或 `{name, image?}`），忽略无 name 成员
pub fn extract_containers(node: &Value) -> Vec<ContainerSpec> {
    let Some(arr) = node.get("containers").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let spec = match item {
            Value::String(name) if !name.is_empty() => Some(ContainerSpec {
                name: name.clone(),
                image: String::new(),
            }),
            Value::Object(map) => map.get("name").and_then(Value::as_str).and_then(|name| {
                if name.is_empty() {
                    return None;
                }
                Some(ContainerSpec {
                    name: name.to_string(),
                    image: map
                        .get("image")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
            }),
            _ => None,
        };
        if let Some(spec) = spec {
            out.push(spec);
        }
    }
    out
}

/// 容器条目
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_ignores_non_prefixed_lines() {
        let out = extract_directives("hello\n  modelinfer:a=1\nother modelinfer:b=2\n");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0],
            Directive::Set {
                path: vec![PathSeg::Key("a".into())],
                value: json!("1")
            }
        );
    }

    #[test]
    fn simplified_operators() {
        assert_eq!(
            parse_simplified("name=Qwen3").unwrap(),
            Directive::Set { path: vec![PathSeg::Key("name".into())], value: json!("Qwen3") }
        );
        assert_eq!(
            parse_simplified("max_len:=32768").unwrap(),
            Directive::SetJson { path: vec![PathSeg::Key("max_len".into())], value: json!(32768) }
        );
        assert_eq!(
            parse_simplified("flag:=true").unwrap(),
            Directive::SetJson { path: vec![PathSeg::Key("flag".into())], value: json!(true) }
        );
        // `:=` 非法 JSON → 字符串
        assert_eq!(
            parse_simplified("x:={oops").unwrap(),
            Directive::SetJson { path: vec![PathSeg::Key("x".into())], value: json!("{oops") }
        );
        assert_eq!(
            parse_simplified("ranks+=0").unwrap(),
            Directive::Append { path: vec![PathSeg::Key("ranks".into())], value: json!(0) }
        );
        assert_eq!(
            parse_simplified("ranks-=0").unwrap(),
            Directive::RemoveValue { path: vec![PathSeg::Key("ranks".into())], value: json!(0) }
        );
    }

    #[test]
    fn simplified_nested_path_and_value_with_equals() {
        assert_eq!(
            parse_simplified("a.b.c=1=2").unwrap(),
            Directive::Set {
                path: vec![
                    PathSeg::Key("a".into()),
                    PathSeg::Key("b".into()),
                    PathSeg::Key("c".into())
                ],
                value: json!("1=2")
            }
        );
        assert_eq!(
            parse_simplified("containers.0.name=ctn").unwrap(),
            Directive::Set {
                path: vec![
                    PathSeg::Key("containers".into()),
                    PathSeg::Index(0),
                    PathSeg::Key("name".into())
                ],
                value: json!("ctn")
            }
        );
    }

    #[test]
    fn simplified_invalid_ignored() {
        assert!(parse_simplified("noop").is_none());
        assert!(parse_simplified("=v").is_none());
        assert!(parse_simplified("a b=1").is_none());
        assert!(parse_simplified("?.=1").is_none());
        assert!(parse_simplified("").is_none());
    }

    #[test]
    fn json_patch_array() {
        let d = parse_data(r#"[{"op":"add","path":"/phase","value":"loading"}]"#);
        let agg = aggregate(&d);
        assert_eq!(agg.node, json!({"phase":"loading"}));
    }

    #[test]
    fn json_patch_single_op_and_global_routing() {
        let d = parse_data(r#"{"op":"add","path":"/global/tp","value":4}"#);
        let agg = aggregate(&d);
        assert_eq!(agg.node, json!({}));
        assert_eq!(agg.global_patch, json!({"tp":4}));
    }

    #[test]
    fn json_merge_and_global_split() {
        let d = parse_data(r#"{"global":{"model":"glm"},"rank":1}"#);
        let agg = aggregate(&d);
        assert_eq!(agg.node, json!({"rank":1}));
        assert_eq!(agg.global_patch, json!({"model":"glm"}));
        // 仅 global
        let agg = aggregate(&parse_data(r#"{"global":{"version":"5.3"}}"#));
        assert_eq!(agg.node, json!({}));
        assert_eq!(agg.global_patch, json!({"version":"5.3"}));
    }

    #[test]
    fn unknown_patch_op_ignored() {
        let d = parse_data(r#"[{"op":"move","path":"/a"},{"op":"add","path":"/b","value":1}]"#);
        let agg = aggregate(&d);
        assert_eq!(agg.node, json!({"b":1}));
    }

    #[test]
    fn deep_merge_objects_arrays_null() {
        let mut t = json!({"a":{"x":1},"arr":[1,2],"gone":1});
        deep_merge(&mut t, &json!({"a":{"y":2},"arr":[3],"gone":null,"new":true}));
        assert_eq!(t, json!({"a":{"x":1,"y":2},"arr":[3],"new":true}));
    }

    #[test]
    fn append_and_remove_list() {
        let d = extract_directives("modelinfer:ranks+=0\nmodelinfer:ranks+=1\nmodelinfer:ranks+=2\n");
        let mut agg = aggregate(&d);
        assert_eq!(agg.node, json!({"ranks":[0,1,2]}));
        // 移除 1
        let d = extract_directives("modelinfer:ranks-=1\n");
        for dir in &d {
            apply_directive(&mut agg, dir);
        }
        assert_eq!(agg.node, json!({"ranks":[0,2]}));
    }

    #[test]
    fn patch_add_append_index_and_remove() {
        let d = parse_data(
            r#"[{"op":"add","path":"/list/-","value":"a"},{"op":"add","path":"/list/-","value":"b"},{"op":"remove","path":"/list/0"}]"#,
        );
        let agg = aggregate(&d);
        assert_eq!(agg.node, json!({"list":["b"]}));
    }

    #[test]
    fn invalid_json_line_falls_back_or_ignored() {
        assert!(parse_data("{not json").is_empty());
        assert!(parse_data("???").is_empty());
        assert!(parse_data("").is_empty());
        // JSON 标量（数字 / 字符串）无意义 → 忽略
        assert!(parse_data("123").is_empty());
        assert!(parse_data("\"x\"").is_empty());
    }

    #[test]
    fn containers_extraction() {
        let node = json!({"containers":[{"name":"a","image":"img"}, "b", {"no":"name"}, {"name":""}]});
        let c = extract_containers(&node);
        assert_eq!(
            c,
            vec![
                ContainerSpec { name: "a".into(), image: "img".into() },
                ContainerSpec { name: "b".into(), image: String::new() },
            ]
        );
        assert!(extract_containers(&json!({})).is_empty());
    }

    #[test]
    fn extensibility_unknown_keys_are_kept() {
        // 新增字段不丢失（扩展性）
        let agg = aggregate(&parse_data(r#"{"future":{"a":[1,2]}}"#));
        assert_eq!(agg.node, json!({"future":{"a":[1,2]}}));
    }
}