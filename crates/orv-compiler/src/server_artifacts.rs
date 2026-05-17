use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use orv_hir::{
    origin_id, BinaryOp, HirBlock, HirExpr, HirExprKind, HirFunctionBody, HirObjectField,
    HirPattern, HirProgram, HirStmt, HirStringSegment, HirTypeRef, HirTypeRefKind, UnaryOp,
};

use super::{
    OriginEntry, OriginMap, ServerListenArtifact, ServerListenEnvArtifact, ServerResponseArtifact,
    ServerResponseConditionArtifact, ServerResponseObjectFieldArtifact,
    ServerResponseQueryParamArtifact, ServerResponseRequestBodyArtifact,
    ServerResponseRequestBodyFieldArtifact, ServerResponseRouteParamArtifact, ServerRouteArtifact,
    ServerRoutePolicyArtifact, ServerRuntimeArtifact, SourceBundleArtifact,
    NATIVE_CONDITION_TRIPLE_PRODUCT, SOURCE_BUNDLE_ARTIFACT_VERSION,
};

fn static_integer(expr: &HirExpr) -> Option<i64> {
    match &expr.kind {
        HirExprKind::Integer(value) => value.parse::<i64>().ok(),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => static_integer(expr).map(|value| -value),
        HirExprKind::Paren(expr) => static_integer(expr),
        _ => None,
    }
}

fn static_float(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Integer(value) | HirExprKind::Float(value) => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|_| value.clone()),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => static_float(expr)
            .and_then(|value| (!value.starts_with('-')).then(|| format!("-{value}"))),
        HirExprKind::Paren(expr) => static_float(expr),
        _ => None,
    }
}

fn static_json_payload(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Integer(value) => value.parse::<i64>().ok().map(|value| value.to_string()),
        HirExprKind::Float(value) => value.parse::<f64>().ok().map(|_| value.clone()),
        HirExprKind::String(segments) => static_string_segments(segments).map(|value| {
            let mut out = String::new();
            write_json_string(&value, &mut out);
            out
        }),
        HirExprKind::True => Some("true".to_string()),
        HirExprKind::False => Some("false".to_string()),
        HirExprKind::Void => Some("null".to_string()),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => static_json_payload(expr).and_then(|value| {
            if value.starts_with('-')
                || !(value.bytes().all(|byte| byte.is_ascii_digit()) || value.contains('.'))
            {
                None
            } else {
                Some(format!("-{value}"))
            }
        }),
        HirExprKind::Paren(expr) => static_json_payload(expr),
        HirExprKind::Array(items) | HirExprKind::Tuple(items) => {
            let mut out = String::from("[");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&static_json_payload(item)?);
            }
            out.push(']');
            Some(out)
        }
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = String::from("{");
            for (index, field) in fields.iter().enumerate() {
                if field.is_spread {
                    return None;
                }
                if index > 0 {
                    out.push(',');
                }
                write_json_string(&field.name, &mut out);
                out.push(':');
                out.push_str(&static_json_payload(&field.value)?);
            }
            out.push('}');
            Some(out)
        }
        _ => None,
    }
}

fn response_status_disallows_body(status: i64) -> bool {
    (100..=199).contains(&status) || status == 204 || status == 304
}

fn response_payload_is_void(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Void => true,
        HirExprKind::Paren(expr) => response_payload_is_void(expr),
        _ => false,
    }
}

fn server_response_artifact(
    expr: &HirExpr,
    status: &HirExpr,
    payload: &HirExpr,
    condition: Option<ServerResponseConditionArtifact>,
) -> ServerResponseArtifact {
    let status_value = static_integer(status);
    let static_payload_json = static_json_payload(payload);
    let body_empty = response_payload_is_void(payload)
        || (status_value.is_some_and(response_status_disallows_body)
            && static_payload_json.is_some());
    let body_json = (!body_empty).then_some(static_payload_json).flatten();
    let body_object_fields = if body_json.is_none() && !body_empty {
        mixed_object_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_route_params = if body_json.is_none() && body_object_fields.is_empty() && !body_empty {
        route_param_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_query_params = if body_json.is_none()
        && body_object_fields.is_empty()
        && body_route_params.is_empty()
        && !body_empty
    {
        query_param_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_request_json = if body_json.is_none()
        && body_object_fields.is_empty()
        && body_route_params.is_empty()
        && body_query_params.is_empty()
        && !body_empty
    {
        request_body_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_request_fields = if body_json.is_none()
        && body_object_fields.is_empty()
        && body_route_params.is_empty()
        && body_query_params.is_empty()
        && body_request_json.is_empty()
        && !body_empty
    {
        request_body_field_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_kind = if body_empty {
        "empty"
    } else if body_json.is_some() {
        "static_json"
    } else if !body_object_fields.is_empty() {
        "mixed_json"
    } else if !body_route_params.is_empty() {
        "route_param_json"
    } else if !body_query_params.is_empty() {
        "query_param_json"
    } else if !body_request_json.is_empty() {
        "request_body_json"
    } else if !body_request_fields.is_empty() {
        "request_body_field_json"
    } else {
        "dynamic"
    };
    ServerResponseArtifact {
        origin_id: origin_id("domain", "respond", expr.span),
        status: status_value,
        body_kind: body_kind.to_string(),
        condition,
        body_json,
        body_object_fields,
        body_route_params,
        body_query_params,
        body_request_json,
        body_request_fields,
    }
}

fn route_param_json_payload(expr: &HirExpr) -> Option<Vec<ServerResponseRouteParamArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            for field in fields {
                if field.is_spread {
                    return None;
                }
                let value = route_param_field_value(&field.value)?;
                out.push(ServerResponseRouteParamArtifact {
                    field: field.name.clone(),
                    param: value.name,
                    value_kind: value.value_kind,
                    op: value.op,
                    operand_json: value.operand_json,
                    operand_kind: value.operand_kind,
                    operand_name: value.operand_name,
                    secondary_operand_kind: value.secondary_operand_kind,
                    secondary_operand_name: value.secondary_operand_name,
                    tertiary_operand_kind: value.tertiary_operand_kind,
                    tertiary_operand_name: value.tertiary_operand_name,
                });
            }
            Some(out)
        }
        HirExprKind::Paren(expr) => route_param_json_payload(expr),
        _ => None,
    }
}

fn query_param_json_payload(expr: &HirExpr) -> Option<Vec<ServerResponseQueryParamArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            for field in fields {
                if field.is_spread {
                    return None;
                }
                let value = query_param_field_value(&field.value)?;
                out.push(ServerResponseQueryParamArtifact {
                    field: field.name.clone(),
                    param: value.name,
                    value_kind: value.value_kind,
                    op: value.op,
                    operand_json: value.operand_json,
                    operand_kind: value.operand_kind,
                    operand_name: value.operand_name,
                    secondary_operand_kind: value.secondary_operand_kind,
                    secondary_operand_name: value.secondary_operand_name,
                    tertiary_operand_kind: value.tertiary_operand_kind,
                    tertiary_operand_name: value.tertiary_operand_name,
                });
            }
            Some(out)
        }
        HirExprKind::Paren(expr) => query_param_json_payload(expr),
        _ => None,
    }
}

fn request_body_json_payload(expr: &HirExpr) -> Option<Vec<ServerResponseRequestBodyArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            for field in fields {
                if field.is_spread || !is_request_body_domain(&field.value) {
                    return None;
                }
                out.push(ServerResponseRequestBodyArtifact {
                    field: field.name.clone(),
                });
            }
            Some(out)
        }
        HirExprKind::Paren(expr) => request_body_json_payload(expr),
        _ => None,
    }
}

fn request_body_field_json_payload(
    expr: &HirExpr,
) -> Option<Vec<ServerResponseRequestBodyFieldArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            for field in fields {
                if field.is_spread {
                    return None;
                }
                let value = request_body_field_value(&field.value)?;
                out.push(ServerResponseRequestBodyFieldArtifact {
                    field: field.name.clone(),
                    name: value.name,
                    value_kind: value.value_kind,
                    op: value.op,
                    operand_json: value.operand_json,
                    operand_kind: value.operand_kind,
                    operand_name: value.operand_name,
                    secondary_operand_kind: value.secondary_operand_kind,
                    secondary_operand_name: value.secondary_operand_name,
                    tertiary_operand_kind: value.tertiary_operand_kind,
                    tertiary_operand_name: value.tertiary_operand_name,
                });
            }
            Some(out)
        }
        HirExprKind::Paren(expr) => request_body_field_json_payload(expr),
        _ => None,
    }
}

fn mixed_object_json_payload(expr: &HirExpr) -> Option<Vec<ServerResponseObjectFieldArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            let mut has_static = false;
            let mut has_dynamic = false;
            let mut first_source: Option<ResponseObjectFieldSource> = None;
            let mut has_mixed_source = false;
            for field in fields {
                if field.is_spread {
                    return None;
                }
                let (artifact, source) =
                    mixed_response_object_field(field, &mut has_static, &mut has_dynamic)?;
                if let Some(first_source) = first_source {
                    has_mixed_source |= first_source != source;
                } else {
                    first_source = Some(source);
                }
                out.push(artifact);
            }
            (has_dynamic && (has_static || has_mixed_source)).then_some(out)
        }
        HirExprKind::Paren(expr) => mixed_object_json_payload(expr),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponseObjectFieldSource {
    Static,
    RouteParam,
    QueryParam,
    RequestBody,
    RequestBodyField,
}

fn mixed_response_object_field(
    field: &HirObjectField,
    has_static: &mut bool,
    has_dynamic: &mut bool,
) -> Option<(ServerResponseObjectFieldArtifact, ResponseObjectFieldSource)> {
    if let Some(value_json) = static_json_payload(&field.value) {
        *has_static = true;
        return Some((
            ServerResponseObjectFieldArtifact {
                field: field.name.clone(),
                value_kind: "static_json".to_string(),
                value_json: Some(value_json),
                name: None,
                op: None,
                operand_json: None,
                operand_kind: None,
                operand_name: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            },
            ResponseObjectFieldSource::Static,
        ));
    }
    if let Some(value) = route_param_field_value(&field.value) {
        *has_dynamic = true;
        return Some((
            ServerResponseObjectFieldArtifact {
                field: field.name.clone(),
                value_kind: value.value_kind,
                value_json: None,
                name: Some(value.name),
                op: value.op,
                operand_json: value.operand_json,
                operand_kind: value.operand_kind,
                operand_name: value.operand_name,
                secondary_operand_kind: value.secondary_operand_kind,
                secondary_operand_name: value.secondary_operand_name,
                tertiary_operand_kind: value.tertiary_operand_kind,
                tertiary_operand_name: value.tertiary_operand_name,
            },
            ResponseObjectFieldSource::RouteParam,
        ));
    }
    if let Some(value) = query_param_field_value(&field.value) {
        *has_dynamic = true;
        return Some((
            ServerResponseObjectFieldArtifact {
                field: field.name.clone(),
                value_kind: value.value_kind,
                value_json: None,
                name: Some(value.name),
                op: value.op,
                operand_json: value.operand_json,
                operand_kind: value.operand_kind,
                operand_name: value.operand_name,
                secondary_operand_kind: value.secondary_operand_kind,
                secondary_operand_name: value.secondary_operand_name,
                tertiary_operand_kind: value.tertiary_operand_kind,
                tertiary_operand_name: value.tertiary_operand_name,
            },
            ResponseObjectFieldSource::QueryParam,
        ));
    }
    if is_request_body_domain(&field.value) {
        *has_dynamic = true;
        return Some((
            ServerResponseObjectFieldArtifact {
                field: field.name.clone(),
                value_kind: "request_body_json".to_string(),
                value_json: None,
                name: None,
                op: None,
                operand_json: None,
                operand_kind: None,
                operand_name: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            },
            ResponseObjectFieldSource::RequestBody,
        ));
    }
    if let Some(value) = request_body_field_value(&field.value) {
        *has_dynamic = true;
        return Some((
            ServerResponseObjectFieldArtifact {
                field: field.name.clone(),
                value_kind: value.value_kind,
                value_json: None,
                name: Some(value.name),
                op: value.op,
                operand_json: value.operand_json,
                operand_kind: value.operand_kind,
                operand_name: value.operand_name,
                secondary_operand_kind: value.secondary_operand_kind,
                secondary_operand_name: value.secondary_operand_name,
                tertiary_operand_kind: value.tertiary_operand_kind,
                tertiary_operand_name: value.tertiary_operand_name,
            },
            ResponseObjectFieldSource::RequestBodyField,
        ));
    }
    None
}

fn route_param_field_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Field { target, field, .. } if is_route_param_domain(target) => {
            Some(field.clone())
        }
        HirExprKind::Paren(expr) => route_param_field_name(expr),
        _ => None,
    }
}

fn query_param_field_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Field { target, field, .. } if is_query_param_domain(target) => {
            Some(field.clone())
        }
        HirExprKind::Paren(expr) => query_param_field_name(expr),
        _ => None,
    }
}

fn route_param_field_value(expr: &HirExpr) -> Option<CapturedResponseValue> {
    match &expr.kind {
        HirExprKind::String(segments) => {
            captured_string_interpolation_response_operation(segments, route_param_field_value)
        }
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or
            ) =>
        {
            captured_response_comparison_operation(
                route_param_field_value(lhs),
                || route_param_field_value(rhs),
                *op,
                lhs,
                rhs,
            )
        }
        HirExprKind::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
        } => captured_add_response_operation(
            route_param_field_value(lhs),
            || route_param_field_value(rhs),
            lhs,
            rhs,
        ),
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => captured_mul_response_operation(
            route_param_field_value(lhs),
            || route_param_field_value(rhs),
            lhs,
            rhs,
        ),
        HirExprKind::Binary {
            op: op @ (BinaryOp::Sub | BinaryOp::Div | BinaryOp::Rem | BinaryOp::Pow),
            lhs,
            rhs,
        } => captured_ordered_arithmetic_response_operation(
            route_param_field_value(lhs),
            || route_param_field_value(rhs),
            *op,
            lhs,
            rhs,
        ),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => captured_numeric_neg_response_operation(route_param_field_value(expr)?),
        HirExprKind::Unary {
            op: UnaryOp::Not,
            expr,
        } => captured_bool_response_operation(route_param_field_value(expr)?),
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => Some(captured_response_value(
            route_param_field_name(expr)?,
            "route_param_int",
        )),
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => Some(captured_response_value(
            route_param_field_name(expr)?,
            "route_param_float",
        )),
        HirExprKind::Cast { expr, ty } if is_bool_type_ref(ty) => Some(captured_response_value(
            route_param_field_name(expr)?,
            "route_param_bool",
        )),
        HirExprKind::Paren(expr) => route_param_field_value(expr),
        _ => Some(captured_response_value(
            route_param_field_name(expr)?,
            "route_param",
        )),
    }
}

fn query_param_field_value(expr: &HirExpr) -> Option<CapturedResponseValue> {
    match &expr.kind {
        HirExprKind::String(segments) => {
            captured_string_interpolation_response_operation(segments, query_param_field_value)
        }
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or
            ) =>
        {
            captured_response_comparison_operation(
                query_param_field_value(lhs),
                || query_param_field_value(rhs),
                *op,
                lhs,
                rhs,
            )
        }
        HirExprKind::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
        } => captured_add_response_operation(
            query_param_field_value(lhs),
            || query_param_field_value(rhs),
            lhs,
            rhs,
        ),
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => captured_mul_response_operation(
            query_param_field_value(lhs),
            || query_param_field_value(rhs),
            lhs,
            rhs,
        ),
        HirExprKind::Binary {
            op: op @ (BinaryOp::Sub | BinaryOp::Div | BinaryOp::Rem | BinaryOp::Pow),
            lhs,
            rhs,
        } => captured_ordered_arithmetic_response_operation(
            query_param_field_value(lhs),
            || query_param_field_value(rhs),
            *op,
            lhs,
            rhs,
        ),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => captured_numeric_neg_response_operation(query_param_field_value(expr)?),
        HirExprKind::Unary {
            op: UnaryOp::Not,
            expr,
        } => captured_bool_response_operation(query_param_field_value(expr)?),
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => Some(captured_response_value(
            query_param_field_name(expr)?,
            "query_param_int",
        )),
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => Some(captured_response_value(
            query_param_field_name(expr)?,
            "query_param_float",
        )),
        HirExprKind::Cast { expr, ty } if is_bool_type_ref(ty) => Some(captured_response_value(
            query_param_field_name(expr)?,
            "query_param_bool",
        )),
        HirExprKind::Paren(expr) => query_param_field_value(expr),
        _ => Some(captured_response_value(
            query_param_field_name(expr)?,
            "query_param",
        )),
    }
}

fn request_body_field_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Field { target, field, .. } if is_request_body_domain(target) => {
            Some(field.clone())
        }
        HirExprKind::Paren(expr) => request_body_field_name(expr),
        _ => None,
    }
}

#[derive(Clone)]
struct CapturedResponseValue {
    name: String,
    value_kind: String,
    op: Option<String>,
    operand_json: Option<String>,
    operand_kind: Option<String>,
    operand_name: Option<String>,
    secondary_operand_kind: Option<String>,
    secondary_operand_name: Option<String>,
    tertiary_operand_kind: Option<String>,
    tertiary_operand_name: Option<String>,
}

impl CapturedResponseValue {
    fn has_operation(&self) -> bool {
        self.op.is_some()
            || self.operand_json.is_some()
            || self.operand_kind.is_some()
            || self.operand_name.is_some()
            || self.secondary_operand_kind.is_some()
            || self.secondary_operand_name.is_some()
            || self.tertiary_operand_kind.is_some()
            || self.tertiary_operand_name.is_some()
    }
}

fn captured_response_value(name: String, value_kind: &str) -> CapturedResponseValue {
    CapturedResponseValue {
        name,
        value_kind: value_kind.to_string(),
        op: None,
        operand_json: None,
        operand_kind: None,
        operand_name: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    }
}

fn captured_response_value_with_product_operand(
    mut value: CapturedResponseValue,
    op: &str,
    operand_kind: String,
    operand_name: String,
    secondary_operand_kind: String,
    secondary_operand_name: String,
) -> CapturedResponseValue {
    value.op = Some(op.to_string());
    value.operand_kind = Some(operand_kind);
    value.operand_name = Some(operand_name);
    value.secondary_operand_kind = Some(secondary_operand_kind);
    value.secondary_operand_name = Some(secondary_operand_name);
    value
}

fn captured_response_value_with_scaled_product_operand(
    mut value: CapturedResponseValue,
    op: &str,
    scale: String,
    operand_kind: String,
    operand_name: String,
    secondary_operand_kind: String,
    secondary_operand_name: String,
) -> CapturedResponseValue {
    value.op = Some(op.to_string());
    value.operand_json = Some(scale);
    value.operand_kind = Some(operand_kind);
    value.operand_name = Some(operand_name);
    value.secondary_operand_kind = Some(secondary_operand_kind);
    value.secondary_operand_name = Some(secondary_operand_name);
    value
}

fn captured_response_value_with_product_product_operands(
    mut value: CapturedResponseValue,
    op: &str,
    secondary_operand_kind: String,
    secondary_operand_name: String,
    tertiary_operand_kind: String,
    tertiary_operand_name: String,
) -> CapturedResponseValue {
    value.op = Some(op.to_string());
    value.secondary_operand_kind = Some(secondary_operand_kind);
    value.secondary_operand_name = Some(secondary_operand_name);
    value.tertiary_operand_kind = Some(tertiary_operand_kind);
    value.tertiary_operand_name = Some(tertiary_operand_name);
    value
}

fn captured_response_value_with_triple_product_operand(
    mut value: CapturedResponseValue,
    op: &str,
    operand: CapturedTripleProductOperand,
) -> CapturedResponseValue {
    value.op = Some(op.to_string());
    value.operand_kind = Some(operand.first_value_kind);
    value.operand_name = Some(operand.first_name);
    value.secondary_operand_kind = Some(operand.second_value_kind);
    value.secondary_operand_name = Some(operand.second_name);
    value.tertiary_operand_kind = Some(operand.third_value_kind);
    value.tertiary_operand_name = Some(operand.third_name);
    value
}

fn captured_response_value_is_plain_product(value: &CapturedResponseValue) -> bool {
    matches!(value.op.as_deref(), Some("mul"))
        && value.operand_json.is_none()
        && value.operand_kind.is_some()
        && value.operand_name.is_some()
        && value.secondary_operand_kind.is_none()
        && value.secondary_operand_name.is_none()
        && value.tertiary_operand_kind.is_none()
        && value.tertiary_operand_name.is_none()
        && (value.value_kind.ends_with("_int") || value.value_kind.ends_with("_float"))
}

fn captured_product_static_right_op_name(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("add_product_static"),
        BinaryOp::Sub => Some("sub_product_static"),
        BinaryOp::Mul => Some("mul_product_static"),
        BinaryOp::Div => Some("div_product_static"),
        BinaryOp::Rem => Some("rem_product_static"),
        _ => None,
    }
}

fn captured_product_product_arithmetic_op_name(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("add_product_product"),
        BinaryOp::Sub => Some("sub_product_product"),
        BinaryOp::Mul => Some("mul_product_product"),
        BinaryOp::Div => Some("div_product_product"),
        BinaryOp::Rem => Some("rem_product_product"),
        _ => None,
    }
}

fn captured_scaled_product_arithmetic_op_name(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("add_scaled_product"),
        BinaryOp::Sub => Some("sub_scaled_product"),
        BinaryOp::Mul => Some("mul_scaled_product"),
        BinaryOp::Div => Some("div_scaled_product"),
        BinaryOp::Rem => Some("rem_scaled_product"),
        _ => None,
    }
}

fn captured_triple_product_arithmetic_op_name(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("add_triple_product"),
        BinaryOp::Sub => Some("sub_triple_product"),
        BinaryOp::Mul => Some("mul_triple_product"),
        BinaryOp::Div => Some("div_triple_product"),
        BinaryOp::Rem => Some("rem_triple_product"),
        _ => None,
    }
}

fn captured_product_static_left_op_name(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("add_product_static"),
        BinaryOp::Sub => Some("rsub_product_static"),
        BinaryOp::Mul => Some("mul_product_static"),
        BinaryOp::Div => Some("rdiv_product_static"),
        BinaryOp::Rem => Some("rrem_product_static"),
        _ => None,
    }
}

fn captured_product_static_response_operation(
    value: &CapturedResponseValue,
    op_name: &str,
    static_operand: &HirExpr,
) -> Option<CapturedResponseValue> {
    if !captured_response_value_is_plain_product(value) {
        return None;
    }
    let mut value = value.clone();
    value.op = Some(op_name.to_string());
    if value.value_kind.ends_with("_int") {
        value.operand_json = Some(static_integer(static_operand)?.to_string());
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        value.operand_json = Some(static_float(static_operand)?);
        return Some(value);
    }
    None
}

fn captured_product_static_left_response_operation(
    value: &CapturedResponseValue,
    op: BinaryOp,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    captured_product_static_response_operation(
        value,
        captured_product_static_left_op_name(op)?,
        lhs,
    )
}

fn captured_product_static_comparison_response_operation(
    value: &CapturedResponseValue,
    op: BinaryOp,
    static_operand: &HirExpr,
) -> Option<CapturedResponseValue> {
    let op_name = captured_numeric_comparison_op_name(op)?;
    let op_name = format!("{op_name}_product_static");
    captured_product_static_response_operation(value, &op_name, static_operand)
}

fn captured_product_product_response_operation(
    value: &CapturedResponseValue,
    op_name: &str,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if !captured_response_value_is_plain_product(value) {
        return None;
    }
    if value.value_kind.ends_with("_int") {
        let operand = captured_product_integer_operand(rhs)?;
        return Some(captured_response_value_with_product_product_operands(
            value.clone(),
            op_name,
            operand.lhs_value_kind,
            operand.lhs_name,
            operand.rhs_value_kind,
            operand.rhs_name,
        ));
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_product_float_operand(rhs)?;
        return Some(captured_response_value_with_product_product_operands(
            value.clone(),
            op_name,
            operand.lhs_value_kind,
            operand.lhs_name,
            operand.rhs_value_kind,
            operand.rhs_name,
        ));
    }
    None
}

fn captured_product_product_comparison_response_operation(
    value: &CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    let op_name = captured_numeric_comparison_op_name(op)?;
    let op_name = format!("{op_name}_product_product");
    captured_product_product_response_operation(value, &op_name, rhs)
}

fn captured_scaled_product_comparison_response_operation(
    value: &CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    let op_name = captured_numeric_comparison_op_name(op)?;
    let op_name = format!("{op_name}_scaled_product");
    captured_scaled_product_response_operation(value, &op_name, rhs)
}

fn captured_triple_product_comparison_response_operation(
    value: &CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    let op_name = captured_numeric_comparison_op_name(op)?;
    let op_name = format!("{op_name}_triple_product");
    captured_triple_product_response_operation(value, &op_name, rhs)
}

fn captured_scaled_product_response_operation(
    value: &CapturedResponseValue,
    op_name: &str,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    if value.value_kind.ends_with("_int") {
        let operand = captured_scaled_product_integer_operand(rhs)?;
        return Some(captured_response_value_with_scaled_product_operand(
            value.clone(),
            op_name,
            operand.scale.to_string(),
            operand.first_value_kind,
            operand.first_name,
            operand.second_value_kind,
            operand.second_name,
        ));
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_scaled_product_float_operand(rhs)?;
        return Some(captured_response_value_with_scaled_product_operand(
            value.clone(),
            op_name,
            operand.scale,
            operand.first_value_kind,
            operand.first_name,
            operand.second_value_kind,
            operand.second_name,
        ));
    }
    None
}

fn captured_triple_product_response_operation(
    value: &CapturedResponseValue,
    op_name: &str,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    if value.value_kind.ends_with("_int") {
        let operand = captured_triple_product_integer_operand(rhs)?;
        return Some(captured_response_value_with_triple_product_operand(
            value.clone(),
            op_name,
            operand,
        ));
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_triple_product_float_operand(rhs)?;
        return Some(captured_response_value_with_triple_product_operand(
            value.clone(),
            op_name,
            operand,
        ));
    }
    None
}

fn captured_numeric_response_operation(
    mut value: CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if let Some(op_name) = captured_product_product_arithmetic_op_name(op) {
        if let Some(value) = captured_product_product_response_operation(&value, op_name, rhs) {
            return Some(value);
        }
    }
    if let Some(op_name) = captured_product_static_right_op_name(op) {
        if let Some(value) = captured_product_static_response_operation(&value, op_name, rhs) {
            return Some(value);
        }
    }
    if let Some(op_name) = captured_scaled_product_arithmetic_op_name(op) {
        if let Some(value) = captured_scaled_product_response_operation(&value, op_name, rhs) {
            return Some(value);
        }
    }
    if let Some(op_name) = captured_triple_product_arithmetic_op_name(op) {
        if let Some(value) = captured_triple_product_response_operation(&value, op_name, rhs) {
            return Some(value);
        }
    }
    if value.has_operation() {
        return None;
    }
    value.op = Some(
        match op {
            BinaryOp::Add => "add",
            BinaryOp::Sub => "sub",
            BinaryOp::Mul => "mul",
            BinaryOp::Div => "div",
            BinaryOp::Rem => "rem",
            BinaryOp::Pow => "pow",
            _ => return None,
        }
        .to_string(),
    );
    if value.value_kind.ends_with("_int") {
        if let Some(operand) = static_integer(rhs) {
            value.operand_json = Some(operand.to_string());
            return Some(value);
        }
        if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
        ) {
            if let Some(operand) = captured_scaled_integer_operand(rhs) {
                value.op = Some(
                    match op {
                        BinaryOp::Add => "add_scaled",
                        BinaryOp::Sub => "sub_scaled",
                        BinaryOp::Mul => "mul_scaled",
                        BinaryOp::Div => "div_scaled",
                        BinaryOp::Rem => "rem_scaled",
                        _ => return None,
                    }
                    .to_string(),
                );
                value.operand_json = Some(operand.scale.to_string());
                value.operand_kind = Some(operand.value_kind);
                value.operand_name = Some(operand.name);
                return Some(value);
            }
        }
        if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
        ) {
            if let Some(operand) = captured_product_integer_operand(rhs) {
                let op_name = match op {
                    BinaryOp::Add => "add_product",
                    BinaryOp::Sub => "sub_product",
                    BinaryOp::Mul => "mul_product",
                    BinaryOp::Div => "div_product",
                    BinaryOp::Rem => "rem_product",
                    _ => return None,
                };
                return Some(captured_response_value_with_product_operand(
                    value,
                    op_name,
                    operand.lhs_value_kind,
                    operand.lhs_name,
                    operand.rhs_value_kind,
                    operand.rhs_name,
                ));
            }
        }
        let operand = captured_integer_operand(rhs)?;
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        if let Some(operand) = static_float(rhs) {
            value.operand_json = Some(operand);
            return Some(value);
        }
        if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
        ) {
            if let Some(operand) = captured_scaled_float_operand(rhs) {
                value.op = Some(
                    match op {
                        BinaryOp::Add => "add_scaled",
                        BinaryOp::Sub => "sub_scaled",
                        BinaryOp::Mul => "mul_scaled",
                        BinaryOp::Div => "div_scaled",
                        BinaryOp::Rem => "rem_scaled",
                        _ => return None,
                    }
                    .to_string(),
                );
                value.operand_json = Some(operand.scale);
                value.operand_kind = Some(operand.value_kind);
                value.operand_name = Some(operand.name);
                return Some(value);
            }
        }
        if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
        ) {
            if let Some(operand) = captured_product_float_operand(rhs) {
                let op_name = match op {
                    BinaryOp::Add => "add_product",
                    BinaryOp::Sub => "sub_product",
                    BinaryOp::Mul => "mul_product",
                    BinaryOp::Div => "div_product",
                    BinaryOp::Rem => "rem_product",
                    _ => return None,
                };
                return Some(captured_response_value_with_product_operand(
                    value,
                    op_name,
                    operand.lhs_value_kind,
                    operand.lhs_name,
                    operand.rhs_value_kind,
                    operand.rhs_name,
                ));
            }
        }
        let operand = captured_float_operand(rhs)?;
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

fn captured_arithmetic_response_operation(
    value: CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if matches!(op, BinaryOp::Add)
        && matches!(
            value.value_kind.as_str(),
            "route_param" | "query_param" | "request_body_field"
        )
    {
        return captured_string_concat_response_operation(value, rhs);
    }
    captured_numeric_response_operation(value, op, rhs)
}

fn captured_add_response_operation(
    lhs_value: Option<CapturedResponseValue>,
    rhs_value: impl FnOnce() -> Option<CapturedResponseValue>,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if let Some(value) = lhs_value {
        if let Some(value) = captured_arithmetic_response_operation(value, BinaryOp::Add, rhs) {
            return Some(value);
        }
    }
    let value = rhs_value()?;
    if matches!(
        value.value_kind.as_str(),
        "route_param" | "query_param" | "request_body_field"
    ) {
        return captured_string_prefix_concat_response_operation(value, lhs);
    }
    if let Some(value) = captured_scaled_left_add_response_operation(&value, lhs) {
        return Some(value);
    }
    if let Some(value) = captured_product_left_add_response_operation(&value, lhs) {
        return Some(value);
    }
    if let Some(value) =
        captured_scaled_product_left_response_operation(&value, lhs, "add_scaled_product")
    {
        return Some(value);
    }
    if let Some(value) =
        captured_triple_product_left_response_operation(&value, lhs, "add_triple_product")
    {
        return Some(value);
    }
    if let Some(value) = captured_product_static_left_response_operation(&value, BinaryOp::Add, lhs)
    {
        return Some(value);
    }
    captured_static_left_numeric_arithmetic_response_operation(value, BinaryOp::Add, lhs)
}

fn captured_ordered_arithmetic_response_operation(
    lhs_value: Option<CapturedResponseValue>,
    rhs_value: impl FnOnce() -> Option<CapturedResponseValue>,
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if let Some(value) = lhs_value {
        if let Some(value) = captured_arithmetic_response_operation(value, op, rhs) {
            return Some(value);
        }
    }
    let value = rhs_value()?;
    let scaled_left = match op {
        BinaryOp::Sub => captured_scaled_left_sub_response_operation(&value, lhs),
        BinaryOp::Div => captured_scaled_left_div_response_operation(&value, lhs),
        BinaryOp::Rem => captured_scaled_left_rem_response_operation(&value, lhs),
        _ => None,
    };
    if let Some(value) = scaled_left {
        return Some(value);
    }
    let product_left = match op {
        BinaryOp::Sub => captured_product_left_response_operation(&value, lhs, "rsub_product"),
        BinaryOp::Div => captured_product_left_response_operation(&value, lhs, "rdiv_product"),
        BinaryOp::Rem => captured_product_left_response_operation(&value, lhs, "rrem_product"),
        _ => None,
    };
    if let Some(value) = product_left {
        return Some(value);
    }
    let scaled_product_left = match op {
        BinaryOp::Sub => {
            captured_scaled_product_left_response_operation(&value, lhs, "rsub_scaled_product")
        }
        BinaryOp::Div => {
            captured_scaled_product_left_response_operation(&value, lhs, "rdiv_scaled_product")
        }
        BinaryOp::Rem => {
            captured_scaled_product_left_response_operation(&value, lhs, "rrem_scaled_product")
        }
        _ => None,
    };
    if let Some(value) = scaled_product_left {
        return Some(value);
    }
    if let Some(value) = captured_product_static_left_response_operation(&value, op, lhs) {
        return Some(value);
    }
    captured_static_left_numeric_arithmetic_response_operation(value, op, lhs)
}

fn captured_mul_response_operation(
    lhs_value: Option<CapturedResponseValue>,
    rhs_value: impl FnOnce() -> Option<CapturedResponseValue>,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if let Some(value) = lhs_value {
        if let Some(value) = captured_arithmetic_response_operation(value, BinaryOp::Mul, rhs) {
            return Some(value);
        }
    }
    let value = rhs_value()?;
    if let Some(value) = captured_scaled_left_mul_response_operation(&value, lhs) {
        return Some(value);
    }
    if let Some(value) = captured_product_left_response_operation(&value, lhs, "mul_product") {
        return Some(value);
    }
    if let Some(value) =
        captured_scaled_product_left_response_operation(&value, lhs, "mul_scaled_product")
    {
        return Some(value);
    }
    if let Some(value) =
        captured_triple_product_left_response_operation(&value, lhs, "mul_triple_product")
    {
        return Some(value);
    }
    if let Some(value) = captured_product_static_left_response_operation(&value, BinaryOp::Mul, lhs)
    {
        return Some(value);
    }
    captured_static_left_numeric_arithmetic_response_operation(value, BinaryOp::Mul, lhs)
}

fn captured_string_concat_response_operation(
    mut value: CapturedResponseValue,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    value.op = Some("concat".to_string());
    if let Some(operand) = static_string_expr(rhs) {
        value.operand_json = Some(operand);
        return Some(value);
    }
    let operand = captured_string_operand(rhs)?;
    value.operand_kind = Some(operand.kind.to_string());
    value.operand_name = Some(operand.name);
    Some(value)
}

fn captured_string_prefix_concat_response_operation(
    mut value: CapturedResponseValue,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation()
        || !matches!(
            value.value_kind.as_str(),
            "route_param" | "query_param" | "request_body_field"
        )
    {
        return None;
    }
    value.op = Some("concat_prefix".to_string());
    value.operand_json = Some(static_string_expr(lhs)?);
    Some(value)
}

fn captured_string_interpolation_response_operation(
    segments: &[HirStringSegment],
    captured_value: fn(&HirExpr) -> Option<CapturedResponseValue>,
) -> Option<CapturedResponseValue> {
    if let Some(value) =
        captured_joined_string_interpolation_response_operation(segments, captured_value)
    {
        return Some(value);
    }
    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut value = None;
    for segment in segments {
        match segment {
            HirStringSegment::Str(text) if value.is_some() => suffix.push_str(text),
            HirStringSegment::Str(text) => prefix.push_str(text),
            HirStringSegment::Interp(expr) if value.is_none() => {
                value = Some(captured_value(expr)?);
            }
            HirStringSegment::Interp(_) => return None,
        }
    }
    let mut value = value?;
    if value.has_operation()
        || !matches!(
            value.value_kind.as_str(),
            "route_param" | "query_param" | "request_body_field"
        )
    {
        return None;
    }
    match (prefix.is_empty(), suffix.is_empty()) {
        (true, true) => Some(value),
        (false, true) => {
            value.op = Some("concat_prefix".to_string());
            value.operand_json = Some(prefix);
            Some(value)
        }
        (true, false) => {
            value.op = Some("concat".to_string());
            value.operand_json = Some(suffix);
            Some(value)
        }
        (false, false) => {
            value.op = Some("concat_affix".to_string());
            value.operand_json = Some(format!("{}:{prefix}{suffix}", prefix.len()));
            Some(value)
        }
    }
}

fn captured_joined_string_interpolation_response_operation(
    segments: &[HirStringSegment],
    captured_value: fn(&HirExpr) -> Option<CapturedResponseValue>,
) -> Option<CapturedResponseValue> {
    let mut before_first = String::new();
    let mut separator = String::new();
    let mut after_second = String::new();
    let mut captures = Vec::new();
    for segment in segments {
        match segment {
            HirStringSegment::Str(text) => match captures.len() {
                0 => before_first.push_str(text),
                1 => separator.push_str(text),
                2 => after_second.push_str(text),
                _ => return None,
            },
            HirStringSegment::Interp(expr) => captures.push(expr.as_ref()),
        }
    }
    if !before_first.is_empty() || !after_second.is_empty() || captures.len() != 2 {
        return None;
    }
    let mut value = captured_value(captures[0])?;
    if value.has_operation()
        || !matches!(
            value.value_kind.as_str(),
            "route_param" | "query_param" | "request_body_field"
        )
    {
        return None;
    }
    let operand = captured_string_operand(captures[1])?;
    value.op = Some("concat_join".to_string());
    value.operand_json = Some(separator);
    value.operand_kind = Some(operand.kind.to_string());
    value.operand_name = Some(operand.name);
    Some(value)
}

fn captured_static_left_numeric_arithmetic_response_operation(
    mut value: CapturedResponseValue,
    op: BinaryOp,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    value.op = Some(
        match op {
            BinaryOp::Add => "add",
            BinaryOp::Sub => "rsub",
            BinaryOp::Mul => "mul",
            BinaryOp::Div => "rdiv",
            BinaryOp::Rem => "rrem",
            BinaryOp::Pow => "rpow",
            _ => return None,
        }
        .to_string(),
    );
    if value.value_kind.ends_with("_int") {
        value.operand_json = Some(static_integer(lhs)?.to_string());
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        value.operand_json = Some(static_float(lhs)?);
        return Some(value);
    }
    None
}

fn captured_scaled_left_add_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    let mut value = captured_response_value(value.name.clone(), &value.value_kind);
    value.op = Some("add_scaled".to_string());
    if value.value_kind.ends_with("_int") {
        let operand = captured_scaled_integer_operand(lhs)?;
        value.operand_json = Some(operand.scale.to_string());
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_scaled_float_operand(lhs)?;
        value.operand_json = Some(operand.scale);
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

fn captured_product_left_add_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    if value.value_kind.ends_with("_int") {
        let operand = captured_product_integer_operand(lhs)?;
        return Some(captured_response_value_with_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            "add_product",
            operand.lhs_value_kind,
            operand.lhs_name,
            operand.rhs_value_kind,
            operand.rhs_name,
        ));
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_product_float_operand(lhs)?;
        return Some(captured_response_value_with_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            "add_product",
            operand.lhs_value_kind,
            operand.lhs_name,
            operand.rhs_value_kind,
            operand.rhs_name,
        ));
    }
    None
}

fn captured_scaled_left_sub_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    let mut value = captured_response_value(value.name.clone(), &value.value_kind);
    value.op = Some("rsub_scaled".to_string());
    if value.value_kind.ends_with("_int") {
        let operand = captured_scaled_integer_operand(lhs)?;
        value.operand_json = Some(operand.scale.to_string());
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_scaled_float_operand(lhs)?;
        value.operand_json = Some(operand.scale);
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

fn captured_product_left_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
    op_name: &str,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    if value.value_kind.ends_with("_int") {
        let operand = captured_product_integer_operand(lhs)?;
        return Some(captured_response_value_with_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            op_name,
            operand.lhs_value_kind,
            operand.lhs_name,
            operand.rhs_value_kind,
            operand.rhs_name,
        ));
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_product_float_operand(lhs)?;
        return Some(captured_response_value_with_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            op_name,
            operand.lhs_value_kind,
            operand.lhs_name,
            operand.rhs_value_kind,
            operand.rhs_name,
        ));
    }
    None
}

fn captured_scaled_product_left_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
    op_name: &str,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    if value.value_kind.ends_with("_int") {
        let operand = captured_scaled_product_integer_operand(lhs)?;
        return Some(captured_response_value_with_scaled_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            op_name,
            operand.scale.to_string(),
            operand.first_value_kind,
            operand.first_name,
            operand.second_value_kind,
            operand.second_name,
        ));
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_scaled_product_float_operand(lhs)?;
        return Some(captured_response_value_with_scaled_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            op_name,
            operand.scale,
            operand.first_value_kind,
            operand.first_name,
            operand.second_value_kind,
            operand.second_name,
        ));
    }
    None
}

fn captured_triple_product_left_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
    op_name: &str,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    if value.value_kind.ends_with("_int") {
        let operand = captured_triple_product_integer_operand(lhs)?;
        return Some(captured_response_value_with_triple_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            op_name,
            operand,
        ));
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_triple_product_float_operand(lhs)?;
        return Some(captured_response_value_with_triple_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            op_name,
            operand,
        ));
    }
    None
}

fn captured_scaled_left_mul_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    let mut value = captured_response_value(value.name.clone(), &value.value_kind);
    value.op = Some("mul_scaled".to_string());
    if value.value_kind.ends_with("_int") {
        let operand = captured_scaled_integer_operand(lhs)?;
        value.operand_json = Some(operand.scale.to_string());
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_scaled_float_operand(lhs)?;
        value.operand_json = Some(operand.scale);
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

fn captured_scaled_left_div_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    let mut value = captured_response_value(value.name.clone(), &value.value_kind);
    value.op = Some("rdiv_scaled".to_string());
    if value.value_kind.ends_with("_int") {
        let operand = captured_scaled_integer_operand(lhs)?;
        value.operand_json = Some(operand.scale.to_string());
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_scaled_float_operand(lhs)?;
        value.operand_json = Some(operand.scale);
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

fn captured_scaled_left_rem_response_operation(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    let mut value = captured_response_value(value.name.clone(), &value.value_kind);
    value.op = Some("rrem_scaled".to_string());
    if value.value_kind.ends_with("_int") {
        let operand = captured_scaled_integer_operand(lhs)?;
        value.operand_json = Some(operand.scale.to_string());
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_scaled_float_operand(lhs)?;
        value.operand_json = Some(operand.scale);
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

fn captured_scaled_left_comparison_response_operation(
    value: &CapturedResponseValue,
    op: BinaryOp,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    let op_name = captured_numeric_comparison_op_name(op)?;
    let mut value = captured_response_value(value.name.clone(), &value.value_kind);
    value.op = Some(format!("{op_name}_scaled"));
    if value.value_kind.ends_with("_int") {
        let operand = captured_scaled_integer_operand(lhs)?;
        value.operand_json = Some(operand.scale.to_string());
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_scaled_float_operand(lhs)?;
        value.operand_json = Some(operand.scale);
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

fn captured_product_left_comparison_response_operation(
    value: &CapturedResponseValue,
    op: BinaryOp,
    lhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    let op_name = captured_numeric_comparison_op_name(op)?;
    if value.value_kind.ends_with("_int") {
        let operand = captured_product_integer_operand(lhs)?;
        return Some(captured_response_value_with_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            &format!("{op_name}_product"),
            operand.lhs_value_kind,
            operand.lhs_name,
            operand.rhs_value_kind,
            operand.rhs_name,
        ));
    }
    if value.value_kind.ends_with("_float") {
        let operand = captured_product_float_operand(lhs)?;
        return Some(captured_response_value_with_product_operand(
            captured_response_value(value.name.clone(), &value.value_kind),
            &format!("{op_name}_product"),
            operand.lhs_value_kind,
            operand.lhs_name,
            operand.rhs_value_kind,
            operand.rhs_name,
        ));
    }
    None
}

fn captured_numeric_neg_response_operation(
    mut value: CapturedResponseValue,
) -> Option<CapturedResponseValue> {
    if value.has_operation()
        || !(value.value_kind.ends_with("_int") || value.value_kind.ends_with("_float"))
    {
        return None;
    }
    value.op = Some("neg".to_string());
    Some(value)
}

fn captured_comparison_response_operation(
    value: CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if matches!(
        value.value_kind.as_str(),
        "route_param" | "query_param" | "request_body_field"
    ) {
        return captured_string_comparison_response_operation(value, op, rhs);
    }
    if value.value_kind.ends_with("_bool") {
        return captured_bool_comparison_response_operation(value, op, rhs);
    }
    captured_numeric_comparison_response_operation(value, op, rhs)
}

fn captured_response_comparison_operation(
    lhs_value: Option<CapturedResponseValue>,
    rhs_value: impl FnOnce() -> Option<CapturedResponseValue>,
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if let Some(value) = lhs_value {
        if let Some(value) = captured_product_product_comparison_response_operation(&value, op, rhs)
        {
            return Some(value);
        }
        if let Some(value) = captured_product_static_comparison_response_operation(&value, op, rhs)
        {
            return Some(value);
        }
        if let Some(value) = captured_scaled_product_comparison_response_operation(&value, op, rhs)
        {
            return Some(value);
        }
        if let Some(value) = captured_triple_product_comparison_response_operation(&value, op, rhs)
        {
            return Some(value);
        }
        if let Some(value) = captured_comparison_response_operation(value, op, rhs) {
            return Some(value);
        }
    }
    let value = rhs_value()?;
    if let Some(value) = captured_product_static_comparison_response_operation(
        &value,
        reverse_comparison_op(op)?,
        lhs,
    ) {
        return Some(value);
    }
    if let Some(value) =
        captured_scaled_left_comparison_response_operation(&value, reverse_comparison_op(op)?, lhs)
    {
        return Some(value);
    }
    if let Some(value) =
        captured_product_left_comparison_response_operation(&value, reverse_comparison_op(op)?, lhs)
    {
        return Some(value);
    }
    let reverse_op_name = captured_numeric_comparison_op_name(reverse_comparison_op(op)?)?;
    if let Some(value) = captured_scaled_product_left_response_operation(
        &value,
        lhs,
        &format!("{reverse_op_name}_scaled_product"),
    ) {
        return Some(value);
    }
    if let Some(value) = captured_triple_product_left_response_operation(
        &value,
        lhs,
        &format!("{reverse_op_name}_triple_product"),
    ) {
        return Some(value);
    }
    if !captured_static_comparison_lhs_is_supported(&value, lhs) {
        return None;
    }
    captured_comparison_response_operation(value, reverse_comparison_op(op)?, lhs)
}

fn captured_static_comparison_lhs_is_supported(
    value: &CapturedResponseValue,
    lhs: &HirExpr,
) -> bool {
    if matches!(
        value.value_kind.as_str(),
        "route_param" | "query_param" | "request_body_field"
    ) {
        return static_string_expr(lhs).is_some();
    }
    if value.value_kind.ends_with("_int") {
        return static_integer(lhs).is_some();
    }
    if value.value_kind.ends_with("_float") {
        return static_float(lhs).is_some();
    }
    value.value_kind.ends_with("_bool") && static_bool(lhs).is_some()
}

fn captured_string_comparison_response_operation(
    mut value: CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    value.op = Some(
        match op {
            BinaryOp::Eq => "eq",
            BinaryOp::Ne => "ne",
            _ => return None,
        }
        .to_string(),
    );
    if let Some(operand) = static_string_expr(rhs) {
        value.operand_json = Some(operand);
        return Some(value);
    }
    let operand = captured_string_operand(rhs)?;
    value.operand_kind = Some(operand.kind.to_string());
    value.operand_name = Some(operand.name);
    Some(value)
}

fn captured_numeric_comparison_response_operation(
    mut value: CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.has_operation() {
        return None;
    }
    let op_name = captured_numeric_comparison_op_name(op)?;
    value.op = Some(op_name.to_string());
    if value.value_kind.ends_with("_int") {
        if let Some(operand) = static_integer(rhs) {
            value.operand_json = Some(operand.to_string());
            return Some(value);
        }
        if let Some(operand) = captured_scaled_integer_operand(rhs) {
            value.op = Some(format!("{op_name}_scaled"));
            value.operand_json = Some(operand.scale.to_string());
            value.operand_kind = Some(operand.value_kind);
            value.operand_name = Some(operand.name);
            return Some(value);
        }
        if let Some(operand) = captured_product_integer_operand(rhs) {
            return Some(captured_response_value_with_product_operand(
                value,
                &format!("{op_name}_product"),
                operand.lhs_value_kind,
                operand.lhs_name,
                operand.rhs_value_kind,
                operand.rhs_name,
            ));
        }
        let operand = captured_integer_operand(rhs)?;
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        if let Some(operand) = static_float(rhs) {
            value.operand_json = Some(operand);
            return Some(value);
        }
        if let Some(operand) = captured_scaled_float_operand(rhs) {
            value.op = Some(format!("{op_name}_scaled"));
            value.operand_json = Some(operand.scale);
            value.operand_kind = Some(operand.value_kind);
            value.operand_name = Some(operand.name);
            return Some(value);
        }
        if let Some(operand) = captured_product_float_operand(rhs) {
            return Some(captured_response_value_with_product_operand(
                value,
                &format!("{op_name}_product"),
                operand.lhs_value_kind,
                operand.lhs_name,
                operand.rhs_value_kind,
                operand.rhs_name,
            ));
        }
        let operand = captured_float_operand(rhs)?;
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

fn captured_numeric_comparison_op_name(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Eq => Some("eq"),
        BinaryOp::Ne => Some("ne"),
        BinaryOp::Lt => Some("lt"),
        BinaryOp::Le => Some("le"),
        BinaryOp::Gt => Some("gt"),
        BinaryOp::Ge => Some("ge"),
        _ => None,
    }
}

fn captured_bool_response_operation(
    mut value: CapturedResponseValue,
) -> Option<CapturedResponseValue> {
    if value.has_operation() || !value.value_kind.ends_with("_bool") {
        return None;
    }
    value.op = Some("not".to_string());
    Some(value)
}

fn captured_bool_comparison_response_operation(
    mut value: CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.operand_json.is_some()
        || value.operand_kind.is_some()
        || value.operand_name.is_some()
        || value.secondary_operand_kind.is_some()
        || value.secondary_operand_name.is_some()
        || !value.value_kind.ends_with("_bool")
    {
        return None;
    }
    let op_name = match (value.op.as_deref(), op) {
        (None, BinaryOp::Eq) => "eq",
        (None, BinaryOp::Ne) => "ne",
        (None, BinaryOp::And) => "and",
        (None, BinaryOp::Or) => "or",
        (Some("not"), BinaryOp::And) => "not_and",
        (Some("not"), BinaryOp::Or) => "not_or",
        _ => return None,
    };
    if value.op.is_some() && !matches!(op_name, "not_and" | "not_or") {
        return None;
    }
    value.op = Some(op_name.to_string());
    if let Some(value_json) = static_bool(rhs).map(|value| value.to_string()) {
        value.operand_json = Some(value_json);
        return Some(value);
    }
    if value
        .op
        .as_deref()
        .is_some_and(|op| matches!(op, "and" | "or"))
    {
        if let Some(operand) = captured_negated_bool_operand(rhs) {
            value.op = Some(
                match op {
                    BinaryOp::And => "and_not",
                    BinaryOp::Or => "or_not",
                    _ => return None,
                }
                .to_string(),
            );
            value.operand_kind = Some(operand.value_kind);
            value.operand_name = Some(operand.name);
            return Some(value);
        }
    }
    let operand = captured_bool_operand(rhs)?;
    value.operand_kind = Some(operand.value_kind);
    value.operand_name = Some(operand.name);
    Some(value)
}

struct CapturedIntegerOperand {
    value_kind: String,
    name: String,
}

struct CapturedScaledIntegerOperand {
    value_kind: String,
    name: String,
    scale: i64,
}

struct CapturedProductIntegerOperand {
    lhs_value_kind: String,
    lhs_name: String,
    rhs_value_kind: String,
    rhs_name: String,
}

struct CapturedScaledProductIntegerOperand {
    first_value_kind: String,
    first_name: String,
    second_value_kind: String,
    second_name: String,
    scale: i64,
}

struct CapturedTripleProductOperand {
    first_value_kind: String,
    first_name: String,
    second_value_kind: String,
    second_name: String,
    third_value_kind: String,
    third_name: String,
}

struct CapturedFloatOperand {
    value_kind: String,
    name: String,
}

struct CapturedScaledFloatOperand {
    value_kind: String,
    name: String,
    scale: String,
}

struct CapturedProductFloatOperand {
    lhs_value_kind: String,
    lhs_name: String,
    rhs_value_kind: String,
    rhs_name: String,
}

struct CapturedScaledProductFloatOperand {
    first_value_kind: String,
    first_name: String,
    second_value_kind: String,
    second_name: String,
    scale: String,
}

struct CapturedBoolOperand {
    value_kind: String,
    name: String,
}

fn captured_string_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    if let Some(name) = request_body_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "request_body_field",
            name,
        });
    }
    if let Some(name) = route_param_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "route_param",
            name,
        });
    }
    if let Some(name) = query_param_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "query_param",
            name,
        });
    }
    None
}

fn captured_integer_operand(expr: &HirExpr) -> Option<CapturedIntegerOperand> {
    if let Some(name) = captured_route_param_integer_name(expr) {
        return Some(CapturedIntegerOperand {
            value_kind: "route_param_int".to_string(),
            name,
        });
    }
    if let Some(name) = captured_query_param_integer_name(expr) {
        return Some(CapturedIntegerOperand {
            value_kind: "query_param_int".to_string(),
            name,
        });
    }
    if let Some(name) = captured_request_body_field_integer_name(expr) {
        return Some(CapturedIntegerOperand {
            value_kind: "request_body_field_int".to_string(),
            name,
        });
    }
    None
}

fn captured_scaled_integer_operand(expr: &HirExpr) -> Option<CapturedScaledIntegerOperand> {
    match &expr.kind {
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            if let (Some(operand), Some(scale)) =
                (captured_integer_operand(lhs), static_integer(rhs))
            {
                return Some(CapturedScaledIntegerOperand {
                    value_kind: operand.value_kind,
                    name: operand.name,
                    scale,
                });
            }
            let scale = static_integer(lhs)?;
            let operand = captured_integer_operand(rhs)?;
            Some(CapturedScaledIntegerOperand {
                value_kind: operand.value_kind,
                name: operand.name,
                scale,
            })
        }
        HirExprKind::Paren(expr) => captured_scaled_integer_operand(expr),
        _ => None,
    }
}

fn captured_product_integer_operand(expr: &HirExpr) -> Option<CapturedProductIntegerOperand> {
    match &expr.kind {
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            let lhs = captured_integer_operand(lhs)?;
            let rhs = captured_integer_operand(rhs)?;
            Some(CapturedProductIntegerOperand {
                lhs_value_kind: lhs.value_kind,
                lhs_name: lhs.name,
                rhs_value_kind: rhs.value_kind,
                rhs_name: rhs.name,
            })
        }
        HirExprKind::Paren(expr) => captured_product_integer_operand(expr),
        _ => None,
    }
}

fn captured_scaled_product_integer_operand(
    expr: &HirExpr,
) -> Option<CapturedScaledProductIntegerOperand> {
    let mut operands = Vec::new();
    let mut scales = Vec::new();
    collect_captured_scaled_product_integer_terms(expr, &mut operands, &mut scales)?;
    let [first, second]: [CapturedIntegerOperand; 2] = operands.try_into().ok()?;
    let [scale]: [i64; 1] = scales.try_into().ok()?;
    Some(CapturedScaledProductIntegerOperand {
        first_value_kind: first.value_kind,
        first_name: first.name,
        second_value_kind: second.value_kind,
        second_name: second.name,
        scale,
    })
}

fn collect_captured_scaled_product_integer_terms(
    expr: &HirExpr,
    operands: &mut Vec<CapturedIntegerOperand>,
    scales: &mut Vec<i64>,
) -> Option<()> {
    match &expr.kind {
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            collect_captured_scaled_product_integer_terms(lhs, operands, scales)?;
            collect_captured_scaled_product_integer_terms(rhs, operands, scales)?;
            Some(())
        }
        HirExprKind::Paren(expr) => {
            collect_captured_scaled_product_integer_terms(expr, operands, scales)
        }
        _ => {
            if let Some(operand) = captured_integer_operand(expr) {
                operands.push(operand);
                return Some(());
            }
            scales.push(static_integer(expr)?);
            Some(())
        }
    }
}

fn captured_triple_product_integer_operand(expr: &HirExpr) -> Option<CapturedTripleProductOperand> {
    let mut operands = Vec::new();
    collect_captured_integer_product_operands(expr, &mut operands)?;
    let [first, second, third]: [CapturedIntegerOperand; 3] = operands.try_into().ok()?;
    Some(CapturedTripleProductOperand {
        first_value_kind: first.value_kind,
        first_name: first.name,
        second_value_kind: second.value_kind,
        second_name: second.name,
        third_value_kind: third.value_kind,
        third_name: third.name,
    })
}

fn collect_captured_integer_product_operands(
    expr: &HirExpr,
    operands: &mut Vec<CapturedIntegerOperand>,
) -> Option<()> {
    match &expr.kind {
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            collect_captured_integer_product_operands(lhs, operands)?;
            collect_captured_integer_product_operands(rhs, operands)?;
            Some(())
        }
        HirExprKind::Paren(expr) => collect_captured_integer_product_operands(expr, operands),
        _ => {
            operands.push(captured_integer_operand(expr)?);
            Some(())
        }
    }
}

fn captured_float_operand(expr: &HirExpr) -> Option<CapturedFloatOperand> {
    if let Some(name) = captured_route_param_float_name(expr) {
        return Some(CapturedFloatOperand {
            value_kind: "route_param_float".to_string(),
            name,
        });
    }
    if let Some(name) = captured_query_param_float_name(expr) {
        return Some(CapturedFloatOperand {
            value_kind: "query_param_float".to_string(),
            name,
        });
    }
    if let Some(name) = captured_request_body_field_float_name(expr) {
        return Some(CapturedFloatOperand {
            value_kind: "request_body_field_float".to_string(),
            name,
        });
    }
    None
}

fn captured_scaled_float_operand(expr: &HirExpr) -> Option<CapturedScaledFloatOperand> {
    match &expr.kind {
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            if let (Some(operand), Some(scale)) = (captured_float_operand(lhs), static_float(rhs)) {
                return Some(CapturedScaledFloatOperand {
                    value_kind: operand.value_kind,
                    name: operand.name,
                    scale,
                });
            }
            let scale = static_float(lhs)?;
            let operand = captured_float_operand(rhs)?;
            Some(CapturedScaledFloatOperand {
                value_kind: operand.value_kind,
                name: operand.name,
                scale,
            })
        }
        HirExprKind::Paren(expr) => captured_scaled_float_operand(expr),
        _ => None,
    }
}

fn captured_product_float_operand(expr: &HirExpr) -> Option<CapturedProductFloatOperand> {
    match &expr.kind {
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            let lhs = captured_float_operand(lhs)?;
            let rhs = captured_float_operand(rhs)?;
            Some(CapturedProductFloatOperand {
                lhs_value_kind: lhs.value_kind,
                lhs_name: lhs.name,
                rhs_value_kind: rhs.value_kind,
                rhs_name: rhs.name,
            })
        }
        HirExprKind::Paren(expr) => captured_product_float_operand(expr),
        _ => None,
    }
}

fn captured_scaled_product_float_operand(
    expr: &HirExpr,
) -> Option<CapturedScaledProductFloatOperand> {
    let mut operands = Vec::new();
    let mut scales = Vec::new();
    collect_captured_scaled_product_float_terms(expr, &mut operands, &mut scales)?;
    let [first, second]: [CapturedFloatOperand; 2] = operands.try_into().ok()?;
    let [scale]: [String; 1] = scales.try_into().ok()?;
    Some(CapturedScaledProductFloatOperand {
        first_value_kind: first.value_kind,
        first_name: first.name,
        second_value_kind: second.value_kind,
        second_name: second.name,
        scale,
    })
}

fn collect_captured_scaled_product_float_terms(
    expr: &HirExpr,
    operands: &mut Vec<CapturedFloatOperand>,
    scales: &mut Vec<String>,
) -> Option<()> {
    match &expr.kind {
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            collect_captured_scaled_product_float_terms(lhs, operands, scales)?;
            collect_captured_scaled_product_float_terms(rhs, operands, scales)?;
            Some(())
        }
        HirExprKind::Paren(expr) => {
            collect_captured_scaled_product_float_terms(expr, operands, scales)
        }
        _ => {
            if let Some(operand) = captured_float_operand(expr) {
                operands.push(operand);
                return Some(());
            }
            scales.push(static_float(expr)?);
            Some(())
        }
    }
}

fn captured_triple_product_float_operand(expr: &HirExpr) -> Option<CapturedTripleProductOperand> {
    let mut operands = Vec::new();
    collect_captured_float_product_operands(expr, &mut operands)?;
    let [first, second, third]: [CapturedFloatOperand; 3] = operands.try_into().ok()?;
    Some(CapturedTripleProductOperand {
        first_value_kind: first.value_kind,
        first_name: first.name,
        second_value_kind: second.value_kind,
        second_name: second.name,
        third_value_kind: third.value_kind,
        third_name: third.name,
    })
}

fn collect_captured_float_product_operands(
    expr: &HirExpr,
    operands: &mut Vec<CapturedFloatOperand>,
) -> Option<()> {
    match &expr.kind {
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            collect_captured_float_product_operands(lhs, operands)?;
            collect_captured_float_product_operands(rhs, operands)?;
            Some(())
        }
        HirExprKind::Paren(expr) => collect_captured_float_product_operands(expr, operands),
        _ => {
            operands.push(captured_float_operand(expr)?);
            Some(())
        }
    }
}

fn captured_bool_operand(expr: &HirExpr) -> Option<CapturedBoolOperand> {
    if let Some(name) = captured_route_param_bool_name(expr) {
        return Some(CapturedBoolOperand {
            value_kind: "route_param_bool".to_string(),
            name,
        });
    }
    if let Some(name) = captured_query_param_bool_name(expr) {
        return Some(CapturedBoolOperand {
            value_kind: "query_param_bool".to_string(),
            name,
        });
    }
    if let Some(name) = captured_request_body_field_bool_name(expr) {
        return Some(CapturedBoolOperand {
            value_kind: "request_body_field_bool".to_string(),
            name,
        });
    }
    None
}

fn captured_negated_bool_operand(expr: &HirExpr) -> Option<CapturedBoolOperand> {
    match &expr.kind {
        HirExprKind::Unary {
            op: UnaryOp::Not,
            expr,
        } => captured_bool_operand(expr),
        HirExprKind::Paren(expr) => captured_negated_bool_operand(expr),
        _ => None,
    }
}

fn captured_route_param_integer_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => route_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_route_param_integer_name(expr),
        _ => None,
    }
}

fn captured_route_param_float_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => route_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_route_param_float_name(expr),
        _ => None,
    }
}

fn captured_route_param_bool_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_bool_type_ref(ty) => route_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_route_param_bool_name(expr),
        _ => None,
    }
}

fn captured_query_param_integer_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => query_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_query_param_integer_name(expr),
        _ => None,
    }
}

fn captured_query_param_float_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => query_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_query_param_float_name(expr),
        _ => None,
    }
}

fn captured_query_param_bool_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_bool_type_ref(ty) => query_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_query_param_bool_name(expr),
        _ => None,
    }
}

fn captured_request_body_field_integer_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => request_body_field_name(expr),
        HirExprKind::Paren(expr) => captured_request_body_field_integer_name(expr),
        _ => None,
    }
}

fn captured_request_body_field_float_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => request_body_field_name(expr),
        HirExprKind::Paren(expr) => captured_request_body_field_float_name(expr),
        _ => None,
    }
}

fn captured_request_body_field_bool_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_bool_type_ref(ty) => request_body_field_name(expr),
        HirExprKind::Paren(expr) => captured_request_body_field_bool_name(expr),
        _ => None,
    }
}

fn request_body_field_value(expr: &HirExpr) -> Option<CapturedResponseValue> {
    match &expr.kind {
        HirExprKind::String(segments) => {
            captured_string_interpolation_response_operation(segments, request_body_field_value)
        }
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or
            ) =>
        {
            captured_response_comparison_operation(
                request_body_field_value(lhs),
                || request_body_field_value(rhs),
                *op,
                lhs,
                rhs,
            )
        }
        HirExprKind::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
        } => captured_add_response_operation(
            request_body_field_value(lhs),
            || request_body_field_value(rhs),
            lhs,
            rhs,
        ),
        HirExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => captured_mul_response_operation(
            request_body_field_value(lhs),
            || request_body_field_value(rhs),
            lhs,
            rhs,
        ),
        HirExprKind::Binary {
            op: op @ (BinaryOp::Sub | BinaryOp::Div | BinaryOp::Rem | BinaryOp::Pow),
            lhs,
            rhs,
        } => captured_ordered_arithmetic_response_operation(
            request_body_field_value(lhs),
            || request_body_field_value(rhs),
            *op,
            lhs,
            rhs,
        ),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => captured_numeric_neg_response_operation(request_body_field_value(expr)?),
        HirExprKind::Unary {
            op: UnaryOp::Not,
            expr,
        } => captured_bool_response_operation(request_body_field_value(expr)?),
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => Some(captured_response_value(
            request_body_field_name(expr)?,
            "request_body_field_int",
        )),
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => Some(captured_response_value(
            request_body_field_name(expr)?,
            "request_body_field_float",
        )),
        HirExprKind::Cast { expr, ty } if is_bool_type_ref(ty) => Some(captured_response_value(
            request_body_field_name(expr)?,
            "request_body_field_bool",
        )),
        HirExprKind::Paren(expr) => request_body_field_value(expr),
        _ => Some(captured_response_value(
            request_body_field_name(expr)?,
            "request_body_field",
        )),
    }
}

fn is_float_type_ref(ty: &HirTypeRef) -> bool {
    matches!(
        &ty.kind,
        HirTypeRefKind::Named(name) if matches!(name.as_str(), "float" | "double")
    )
}

fn is_bool_type_ref(ty: &HirTypeRef) -> bool {
    matches!(&ty.kind, HirTypeRefKind::Named(name) if name == "bool")
}

fn is_integer_type_ref(ty: &HirTypeRef) -> bool {
    matches!(
        &ty.kind,
        HirTypeRefKind::Named(name)
            if matches!(
                name.as_str(),
                "int" | "uint" | "byte" | "ubyte" | "short" | "ushort" | "long" | "ulong"
            )
    )
}

fn is_route_param_domain(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Domain { name, args, .. } => name == "param" && args.is_empty(),
        HirExprKind::Paren(expr) => is_route_param_domain(expr),
        _ => false,
    }
}

fn is_query_param_domain(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Domain { name, args, .. } => name == "query" && args.is_empty(),
        HirExprKind::Paren(expr) => is_query_param_domain(expr),
        _ => false,
    }
}

fn is_request_body_domain(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Domain { name, args, .. } => name == "body" && args.is_empty(),
        HirExprKind::Paren(expr) => is_request_body_domain(expr),
        _ => false,
    }
}

fn static_string_segments(segments: &[HirStringSegment]) -> Option<String> {
    let mut out = String::new();
    for segment in segments {
        match segment {
            HirStringSegment::Str(value) => out.push_str(value),
            HirStringSegment::Interp(_) => return None,
        }
    }
    Some(out)
}

fn static_string_expr(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::String(segments) => static_string_segments(segments),
        HirExprKind::Paren(expr) => static_string_expr(expr),
        _ => None,
    }
}

fn static_bool(expr: &HirExpr) -> Option<bool> {
    match &expr.kind {
        HirExprKind::True => Some(true),
        HirExprKind::False => Some(false),
        HirExprKind::Paren(expr) => static_bool(expr),
        _ => None,
    }
}

fn native_response_condition(expr: &HirExpr) -> Option<ServerResponseConditionArtifact> {
    match &expr.kind {
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::And
                    | BinaryOp::Or
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
            ) =>
        {
            native_captured_float_response_condition(*op, lhs, rhs)
                .or_else(|| native_captured_int_response_condition(*op, lhs, rhs))
                .or_else(|| native_captured_bool_response_condition(*op, lhs, rhs))
                .or_else(|| {
                    matches!(op, BinaryOp::Eq | BinaryOp::Ne)
                        .then(|| native_captured_response_condition(*op, lhs, rhs))
                        .flatten()
                })
        }
        HirExprKind::Paren(expr) => native_response_condition(expr),
        HirExprKind::Unary {
            op: UnaryOp::Not,
            expr,
        } => native_captured_bool_negated_response_condition(expr),
        _ => native_captured_bool_truthy_response_condition(expr),
    }
}

fn native_captured_bool_negated_response_condition(
    expr: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    let operand = captured_condition_bool_operand(expr)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_bool_operand(BinaryOp::Ne, operand.kind)?,
        name: operand.name,
        value: "true".to_string(),
        operand_name: None,
        operand_kind: None,
        operand_scale_json: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    })
}

fn native_captured_bool_truthy_response_condition(
    expr: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    let operand = captured_condition_bool_operand(expr)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_bool_operand(BinaryOp::Eq, operand.kind)?,
        name: operand.name,
        value: "true".to_string(),
        operand_name: None,
        operand_kind: None,
        operand_scale_json: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    })
}

fn native_captured_bool_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if !matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::And | BinaryOp::Or
    ) {
        return None;
    }
    if matches!(op, BinaryOp::And | BinaryOp::Or) {
        if let Some(left) = captured_negated_condition_bool_operand(lhs) {
            if let Some(value) = static_bool(rhs) {
                return Some(ServerResponseConditionArtifact {
                    kind: condition_kind_for_negated_bool_operand(op, left.kind)?,
                    name: left.name,
                    value: value.to_string(),
                    operand_name: None,
                    operand_kind: None,
                    operand_scale_json: None,
                    secondary_operand_kind: None,
                    secondary_operand_name: None,
                    tertiary_operand_kind: None,
                    tertiary_operand_name: None,
                });
            }
            let right = captured_condition_bool_operand(rhs)?;
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_negated_bool_operand(op, left.kind)?,
                name: left.name,
                value: String::new(),
                operand_name: Some(right.name),
                operand_kind: Some(right.kind.to_string()),
                operand_scale_json: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
    }
    if let Some(left) = captured_condition_bool_operand(lhs) {
        if let Some(value) = static_bool(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_bool_operand(op, left.kind)?,
                name: left.name,
                value: value.to_string(),
                operand_name: None,
                operand_kind: None,
                operand_scale_json: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            if let Some(right) = captured_negated_condition_bool_operand(rhs) {
                return Some(ServerResponseConditionArtifact {
                    kind: condition_kind_for_right_negated_bool_operand(op, left.kind)?,
                    name: left.name,
                    value: String::new(),
                    operand_name: Some(right.name),
                    operand_kind: Some(right.kind.to_string()),
                    operand_scale_json: None,
                    secondary_operand_kind: None,
                    secondary_operand_name: None,
                    tertiary_operand_kind: None,
                    tertiary_operand_name: None,
                });
            }
        }
        let right = captured_condition_bool_operand(rhs)?;
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_bool_operand(op, left.kind)?,
            name: left.name,
            value: String::new(),
            operand_name: Some(right.name),
            operand_kind: Some(right.kind.to_string()),
            operand_scale_json: None,
            secondary_operand_kind: None,
            secondary_operand_name: None,
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    let right = captured_condition_bool_operand(rhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_bool_operand(op, right.kind)?,
        name: right.name,
        value: static_bool(lhs)?.to_string(),
        operand_name: None,
        operand_kind: None,
        operand_scale_json: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    })
}

fn captured_condition_bool_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    let operand = captured_bool_operand(expr)?;
    let kind = match operand.value_kind.as_str() {
        "request_body_field_bool" => "request_body_field_bool",
        "route_param_bool" => "route_param_bool",
        "query_param_bool" => "query_param_bool",
        _ => return None,
    };
    Some(CapturedConditionOperand {
        kind,
        name: operand.name,
    })
}

fn captured_negated_condition_bool_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    match &expr.kind {
        HirExprKind::Unary {
            op: UnaryOp::Not,
            expr,
        } => captured_condition_bool_operand(expr),
        HirExprKind::Paren(expr) => captured_negated_condition_bool_operand(expr),
        _ => None,
    }
}

fn native_captured_float_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if let Some(condition) = native_captured_product_product_float_response_condition(op, lhs, rhs)
    {
        return Some(condition);
    }
    if let Some(condition) = native_captured_product_static_float_response_condition(op, lhs, rhs) {
        return Some(condition);
    }
    if let Some(left) = captured_condition_float_operand(lhs) {
        if let Some(value) = static_float(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_float_operand(op, left.kind)?,
                name: left.name,
                value,
                operand_name: None,
                operand_kind: None,
                operand_scale_json: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if let Some(right) = captured_scaled_condition_float_operand(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_float_operand(op, left.kind)?,
                name: left.name,
                value: String::new(),
                operand_name: Some(right.name),
                operand_kind: Some(right.kind.to_string()),
                operand_scale_json: Some(right.scale_json),
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if let Some(right) = captured_scaled_product_condition_float_operand(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_float_operand(op, left.kind)?,
                name: left.name,
                value: String::new(),
                operand_name: Some(right.first_name),
                operand_kind: Some(right.first_kind.to_string()),
                operand_scale_json: Some(right.scale_json),
                secondary_operand_kind: Some(right.second_kind.to_string()),
                secondary_operand_name: Some(right.second_name),
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if let Some(right) = captured_product_condition_float_operand(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_float_operand(op, left.kind)?,
                name: left.name,
                value: String::new(),
                operand_name: Some(right.lhs_name),
                operand_kind: Some(right.lhs_kind.to_string()),
                operand_scale_json: None,
                secondary_operand_kind: Some(right.rhs_kind.to_string()),
                secondary_operand_name: Some(right.rhs_name),
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if let Some(right) = captured_triple_product_condition_float_operand(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_float_operand(op, left.kind)?,
                name: left.name,
                value: NATIVE_CONDITION_TRIPLE_PRODUCT.to_string(),
                operand_name: Some(right.first_name),
                operand_kind: Some(right.first_value_kind),
                operand_scale_json: None,
                secondary_operand_kind: Some(right.second_value_kind),
                secondary_operand_name: Some(right.second_name),
                tertiary_operand_kind: Some(right.third_value_kind),
                tertiary_operand_name: Some(right.third_name),
            });
        }
        let right = captured_condition_float_operand(rhs)?;
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_float_operand(op, left.kind)?,
            name: left.name,
            value: String::new(),
            operand_name: Some(right.name),
            operand_kind: Some(right.kind.to_string()),
            operand_scale_json: None,
            secondary_operand_kind: None,
            secondary_operand_name: None,
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    let right = captured_condition_float_operand(rhs)?;
    if let Some(left) = captured_scaled_condition_float_operand(lhs) {
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_float_operand(reverse_comparison_op(op)?, right.kind)?,
            name: right.name,
            value: String::new(),
            operand_name: Some(left.name),
            operand_kind: Some(left.kind.to_string()),
            operand_scale_json: Some(left.scale_json),
            secondary_operand_kind: None,
            secondary_operand_name: None,
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    if let Some(left) = captured_scaled_product_condition_float_operand(lhs) {
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_float_operand(reverse_comparison_op(op)?, right.kind)?,
            name: right.name,
            value: String::new(),
            operand_name: Some(left.first_name),
            operand_kind: Some(left.first_kind.to_string()),
            operand_scale_json: Some(left.scale_json),
            secondary_operand_kind: Some(left.second_kind.to_string()),
            secondary_operand_name: Some(left.second_name),
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    if let Some(left) = captured_product_condition_float_operand(lhs) {
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_float_operand(reverse_comparison_op(op)?, right.kind)?,
            name: right.name,
            value: String::new(),
            operand_name: Some(left.lhs_name),
            operand_kind: Some(left.lhs_kind.to_string()),
            operand_scale_json: None,
            secondary_operand_kind: Some(left.rhs_kind.to_string()),
            secondary_operand_name: Some(left.rhs_name),
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    if let Some(left) = captured_triple_product_condition_float_operand(lhs) {
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_float_operand(reverse_comparison_op(op)?, right.kind)?,
            name: right.name,
            value: NATIVE_CONDITION_TRIPLE_PRODUCT.to_string(),
            operand_name: Some(left.first_name),
            operand_kind: Some(left.first_value_kind),
            operand_scale_json: None,
            secondary_operand_kind: Some(left.second_value_kind),
            secondary_operand_name: Some(left.second_name),
            tertiary_operand_kind: Some(left.third_value_kind),
            tertiary_operand_name: Some(left.third_name),
        });
    }
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_float_operand(reverse_comparison_op(op)?, right.kind)?,
        name: right.name,
        value: static_float(lhs)?,
        operand_name: None,
        operand_kind: None,
        operand_scale_json: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    })
}

fn captured_condition_float_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    let operand = captured_float_operand(expr)?;
    let kind = condition_float_operand_kind(operand.value_kind.as_str())?;
    Some(CapturedConditionOperand {
        kind,
        name: operand.name,
    })
}

fn captured_scaled_condition_float_operand(
    expr: &HirExpr,
) -> Option<CapturedScaledConditionOperand> {
    let operand = captured_scaled_float_operand(expr)?;
    Some(CapturedScaledConditionOperand {
        kind: condition_float_operand_kind(operand.value_kind.as_str())?,
        name: operand.name,
        scale_json: operand.scale,
    })
}

fn captured_product_condition_float_operand(
    expr: &HirExpr,
) -> Option<CapturedProductConditionOperand> {
    let operand = captured_product_float_operand(expr)?;
    Some(CapturedProductConditionOperand {
        lhs_kind: condition_float_operand_kind(operand.lhs_value_kind.as_str())?,
        lhs_name: operand.lhs_name,
        rhs_kind: condition_float_operand_kind(operand.rhs_value_kind.as_str())?,
        rhs_name: operand.rhs_name,
    })
}

fn captured_scaled_product_condition_float_operand(
    expr: &HirExpr,
) -> Option<CapturedScaledProductConditionOperand> {
    let operand = captured_scaled_product_float_operand(expr)?;
    Some(CapturedScaledProductConditionOperand {
        first_kind: condition_float_operand_kind(operand.first_value_kind.as_str())?,
        first_name: operand.first_name,
        second_kind: condition_float_operand_kind(operand.second_value_kind.as_str())?,
        second_name: operand.second_name,
        scale_json: operand.scale,
    })
}

fn captured_triple_product_condition_float_operand(
    expr: &HirExpr,
) -> Option<CapturedTripleProductOperand> {
    let operand = captured_triple_product_float_operand(expr)?;
    Some(CapturedTripleProductOperand {
        first_value_kind: condition_float_operand_kind(operand.first_value_kind.as_str())?
            .to_string(),
        first_name: operand.first_name,
        second_value_kind: condition_float_operand_kind(operand.second_value_kind.as_str())?
            .to_string(),
        second_name: operand.second_name,
        third_value_kind: condition_float_operand_kind(operand.third_value_kind.as_str())?
            .to_string(),
        third_name: operand.third_name,
    })
}

fn native_captured_product_static_float_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if let Some(left) = captured_product_condition_float_operand(lhs) {
        if let Some(value) = static_float(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_float_operand(op, left.lhs_kind)?,
                name: left.lhs_name,
                value,
                operand_name: Some(left.rhs_name),
                operand_kind: Some(left.rhs_kind.to_string()),
                operand_scale_json: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
    }
    let right = captured_product_condition_float_operand(rhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_float_operand(reverse_comparison_op(op)?, right.lhs_kind)?,
        name: right.lhs_name,
        value: static_float(lhs)?,
        operand_name: Some(right.rhs_name),
        operand_kind: Some(right.rhs_kind.to_string()),
        operand_scale_json: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    })
}

fn native_captured_product_product_float_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    let left = captured_product_condition_float_operand(lhs)?;
    let right = captured_product_condition_float_operand(rhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_float_operand(op, left.lhs_kind)?,
        name: left.lhs_name,
        value: String::new(),
        operand_name: Some(left.rhs_name),
        operand_kind: Some(left.rhs_kind.to_string()),
        operand_scale_json: None,
        secondary_operand_kind: Some(right.lhs_kind.to_string()),
        secondary_operand_name: Some(right.lhs_name),
        tertiary_operand_kind: Some(right.rhs_kind.to_string()),
        tertiary_operand_name: Some(right.rhs_name),
    })
}

fn condition_float_operand_kind(value_kind: &str) -> Option<&'static str> {
    match value_kind {
        "request_body_field_float" => Some("request_body_field_float"),
        "route_param_float" => Some("route_param_float"),
        "query_param_float" => Some("query_param_float"),
        _ => None,
    }
}

fn native_captured_int_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if let Some(condition) = native_captured_product_product_int_response_condition(op, lhs, rhs) {
        return Some(condition);
    }
    if let Some(condition) = native_captured_product_static_int_response_condition(op, lhs, rhs) {
        return Some(condition);
    }
    if let Some(left) = captured_condition_int_operand(lhs) {
        if let Some(value) = static_integer(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_int_operand(op, left.kind)?,
                name: left.name,
                value: value.to_string(),
                operand_name: None,
                operand_kind: None,
                operand_scale_json: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if let Some(right) = captured_scaled_condition_int_operand(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_int_operand(op, left.kind)?,
                name: left.name,
                value: String::new(),
                operand_name: Some(right.name),
                operand_kind: Some(right.kind.to_string()),
                operand_scale_json: Some(right.scale_json),
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if let Some(right) = captured_scaled_product_condition_int_operand(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_int_operand(op, left.kind)?,
                name: left.name,
                value: String::new(),
                operand_name: Some(right.first_name),
                operand_kind: Some(right.first_kind.to_string()),
                operand_scale_json: Some(right.scale_json),
                secondary_operand_kind: Some(right.second_kind.to_string()),
                secondary_operand_name: Some(right.second_name),
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if let Some(right) = captured_product_condition_int_operand(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_int_operand(op, left.kind)?,
                name: left.name,
                value: String::new(),
                operand_name: Some(right.lhs_name),
                operand_kind: Some(right.lhs_kind.to_string()),
                operand_scale_json: None,
                secondary_operand_kind: Some(right.rhs_kind.to_string()),
                secondary_operand_name: Some(right.rhs_name),
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        if let Some(right) = captured_triple_product_condition_int_operand(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_int_operand(op, left.kind)?,
                name: left.name,
                value: NATIVE_CONDITION_TRIPLE_PRODUCT.to_string(),
                operand_name: Some(right.first_name),
                operand_kind: Some(right.first_value_kind),
                operand_scale_json: None,
                secondary_operand_kind: Some(right.second_value_kind),
                secondary_operand_name: Some(right.second_name),
                tertiary_operand_kind: Some(right.third_value_kind),
                tertiary_operand_name: Some(right.third_name),
            });
        }
        let right = captured_condition_int_operand(rhs)?;
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_int_operand(op, left.kind)?,
            name: left.name,
            value: String::new(),
            operand_name: Some(right.name),
            operand_kind: Some(right.kind.to_string()),
            operand_scale_json: None,
            secondary_operand_kind: None,
            secondary_operand_name: None,
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    let right = captured_condition_int_operand(rhs)?;
    if let Some(left) = captured_scaled_condition_int_operand(lhs) {
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_int_operand(reverse_comparison_op(op)?, right.kind)?,
            name: right.name,
            value: String::new(),
            operand_name: Some(left.name),
            operand_kind: Some(left.kind.to_string()),
            operand_scale_json: Some(left.scale_json),
            secondary_operand_kind: None,
            secondary_operand_name: None,
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    if let Some(left) = captured_scaled_product_condition_int_operand(lhs) {
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_int_operand(reverse_comparison_op(op)?, right.kind)?,
            name: right.name,
            value: String::new(),
            operand_name: Some(left.first_name),
            operand_kind: Some(left.first_kind.to_string()),
            operand_scale_json: Some(left.scale_json),
            secondary_operand_kind: Some(left.second_kind.to_string()),
            secondary_operand_name: Some(left.second_name),
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    if let Some(left) = captured_product_condition_int_operand(lhs) {
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_int_operand(reverse_comparison_op(op)?, right.kind)?,
            name: right.name,
            value: String::new(),
            operand_name: Some(left.lhs_name),
            operand_kind: Some(left.lhs_kind.to_string()),
            operand_scale_json: None,
            secondary_operand_kind: Some(left.rhs_kind.to_string()),
            secondary_operand_name: Some(left.rhs_name),
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    if let Some(left) = captured_triple_product_condition_int_operand(lhs) {
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_int_operand(reverse_comparison_op(op)?, right.kind)?,
            name: right.name,
            value: NATIVE_CONDITION_TRIPLE_PRODUCT.to_string(),
            operand_name: Some(left.first_name),
            operand_kind: Some(left.first_value_kind),
            operand_scale_json: None,
            secondary_operand_kind: Some(left.second_value_kind),
            secondary_operand_name: Some(left.second_name),
            tertiary_operand_kind: Some(left.third_value_kind),
            tertiary_operand_name: Some(left.third_name),
        });
    }
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_int_operand(reverse_comparison_op(op)?, right.kind)?,
        name: right.name,
        value: static_integer(lhs)?.to_string(),
        operand_name: None,
        operand_kind: None,
        operand_scale_json: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    })
}

fn native_captured_product_static_int_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if let Some(left) = captured_product_condition_int_operand(lhs) {
        if let Some(value) = static_integer(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_int_operand(op, left.lhs_kind)?,
                name: left.lhs_name,
                value: value.to_string(),
                operand_name: Some(left.rhs_name),
                operand_kind: Some(left.rhs_kind.to_string()),
                operand_scale_json: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
    }
    let right = captured_product_condition_int_operand(rhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_int_operand(reverse_comparison_op(op)?, right.lhs_kind)?,
        name: right.lhs_name,
        value: static_integer(lhs)?.to_string(),
        operand_name: Some(right.rhs_name),
        operand_kind: Some(right.rhs_kind.to_string()),
        operand_scale_json: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    })
}

fn native_captured_product_product_int_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    let left = captured_product_condition_int_operand(lhs)?;
    let right = captured_product_condition_int_operand(rhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_int_operand(op, left.lhs_kind)?,
        name: left.lhs_name,
        value: String::new(),
        operand_name: Some(left.rhs_name),
        operand_kind: Some(left.rhs_kind.to_string()),
        operand_scale_json: None,
        secondary_operand_kind: Some(right.lhs_kind.to_string()),
        secondary_operand_name: Some(right.lhs_name),
        tertiary_operand_kind: Some(right.rhs_kind.to_string()),
        tertiary_operand_name: Some(right.rhs_name),
    })
}

fn captured_condition_int_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    let operand = captured_integer_operand(expr)?;
    let kind = condition_int_operand_kind(operand.value_kind.as_str())?;
    Some(CapturedConditionOperand {
        kind,
        name: operand.name,
    })
}

fn captured_scaled_condition_int_operand(expr: &HirExpr) -> Option<CapturedScaledConditionOperand> {
    let operand = captured_scaled_integer_operand(expr)?;
    Some(CapturedScaledConditionOperand {
        kind: condition_int_operand_kind(operand.value_kind.as_str())?,
        name: operand.name,
        scale_json: operand.scale.to_string(),
    })
}

fn captured_product_condition_int_operand(
    expr: &HirExpr,
) -> Option<CapturedProductConditionOperand> {
    let operand = captured_product_integer_operand(expr)?;
    Some(CapturedProductConditionOperand {
        lhs_kind: condition_int_operand_kind(operand.lhs_value_kind.as_str())?,
        lhs_name: operand.lhs_name,
        rhs_kind: condition_int_operand_kind(operand.rhs_value_kind.as_str())?,
        rhs_name: operand.rhs_name,
    })
}

fn captured_scaled_product_condition_int_operand(
    expr: &HirExpr,
) -> Option<CapturedScaledProductConditionOperand> {
    let operand = captured_scaled_product_integer_operand(expr)?;
    Some(CapturedScaledProductConditionOperand {
        first_kind: condition_int_operand_kind(operand.first_value_kind.as_str())?,
        first_name: operand.first_name,
        second_kind: condition_int_operand_kind(operand.second_value_kind.as_str())?,
        second_name: operand.second_name,
        scale_json: operand.scale.to_string(),
    })
}

fn captured_triple_product_condition_int_operand(
    expr: &HirExpr,
) -> Option<CapturedTripleProductOperand> {
    let operand = captured_triple_product_integer_operand(expr)?;
    Some(CapturedTripleProductOperand {
        first_value_kind: condition_int_operand_kind(operand.first_value_kind.as_str())?
            .to_string(),
        first_name: operand.first_name,
        second_value_kind: condition_int_operand_kind(operand.second_value_kind.as_str())?
            .to_string(),
        second_name: operand.second_name,
        third_value_kind: condition_int_operand_kind(operand.third_value_kind.as_str())?
            .to_string(),
        third_name: operand.third_name,
    })
}

fn condition_int_operand_kind(value_kind: &str) -> Option<&'static str> {
    match value_kind {
        "request_body_field_int" => Some("request_body_field_int"),
        "route_param_int" => Some("route_param_int"),
        "query_param_int" => Some("query_param_int"),
        _ => None,
    }
}

const fn reverse_comparison_op(op: BinaryOp) -> Option<BinaryOp> {
    match op {
        BinaryOp::Eq => Some(BinaryOp::Eq),
        BinaryOp::Ne => Some(BinaryOp::Ne),
        BinaryOp::Lt => Some(BinaryOp::Gt),
        BinaryOp::Le => Some(BinaryOp::Ge),
        BinaryOp::Gt => Some(BinaryOp::Lt),
        BinaryOp::Ge => Some(BinaryOp::Le),
        _ => None,
    }
}

fn native_captured_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if let Some(left) = captured_condition_operand(lhs) {
        if let Some(value) = static_string_expr(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_operand(op, left.kind)?,
                name: left.name,
                value,
                operand_name: None,
                operand_kind: None,
                operand_scale_json: None,
                secondary_operand_kind: None,
                secondary_operand_name: None,
                tertiary_operand_kind: None,
                tertiary_operand_name: None,
            });
        }
        let right = captured_condition_operand(rhs)?;
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_operand(op, left.kind)?,
            name: left.name,
            value: String::new(),
            operand_name: Some(right.name),
            operand_kind: Some(right.kind.to_string()),
            operand_scale_json: None,
            secondary_operand_kind: None,
            secondary_operand_name: None,
            tertiary_operand_kind: None,
            tertiary_operand_name: None,
        });
    }
    let right = captured_condition_operand(rhs)?;
    let value = static_string_expr(lhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_operand(op, right.kind)?,
        name: right.name,
        value,
        operand_name: None,
        operand_kind: None,
        operand_scale_json: None,
        secondary_operand_kind: None,
        secondary_operand_name: None,
        tertiary_operand_kind: None,
        tertiary_operand_name: None,
    })
}

struct CapturedConditionOperand {
    kind: &'static str,
    name: String,
}

struct CapturedScaledConditionOperand {
    kind: &'static str,
    name: String,
    scale_json: String,
}

struct CapturedProductConditionOperand {
    lhs_kind: &'static str,
    lhs_name: String,
    rhs_kind: &'static str,
    rhs_name: String,
}

struct CapturedScaledProductConditionOperand {
    first_kind: &'static str,
    first_name: String,
    second_kind: &'static str,
    second_name: String,
    scale_json: String,
}

fn captured_condition_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    if let Some(name) = request_body_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "request_body_field",
            name,
        });
    }
    if let Some(name) = route_param_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "route_param",
            name,
        });
    }
    if let Some(name) = query_param_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "query_param",
            name,
        });
    }
    None
}

fn condition_kind_for_operand(op: BinaryOp, operand_kind: &str) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field", BinaryOp::Eq) => "request_body_field_eq",
        ("request_body_field", BinaryOp::Ne) => "request_body_field_ne",
        ("route_param", BinaryOp::Eq) => "route_param_eq",
        ("route_param", BinaryOp::Ne) => "route_param_ne",
        ("query_param", BinaryOp::Eq) => "query_param_eq",
        ("query_param", BinaryOp::Ne) => "query_param_ne",
        _ => return None,
    };
    Some(kind.to_string())
}

fn condition_kind_for_int_operand(op: BinaryOp, operand_kind: &str) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field_int", BinaryOp::Eq) => "request_body_field_int_eq",
        ("request_body_field_int", BinaryOp::Ne) => "request_body_field_int_ne",
        ("request_body_field_int", BinaryOp::Lt) => "request_body_field_int_lt",
        ("request_body_field_int", BinaryOp::Le) => "request_body_field_int_le",
        ("request_body_field_int", BinaryOp::Gt) => "request_body_field_int_gt",
        ("request_body_field_int", BinaryOp::Ge) => "request_body_field_int_ge",
        ("route_param_int", BinaryOp::Eq) => "route_param_int_eq",
        ("route_param_int", BinaryOp::Ne) => "route_param_int_ne",
        ("route_param_int", BinaryOp::Lt) => "route_param_int_lt",
        ("route_param_int", BinaryOp::Le) => "route_param_int_le",
        ("route_param_int", BinaryOp::Gt) => "route_param_int_gt",
        ("route_param_int", BinaryOp::Ge) => "route_param_int_ge",
        ("query_param_int", BinaryOp::Eq) => "query_param_int_eq",
        ("query_param_int", BinaryOp::Ne) => "query_param_int_ne",
        ("query_param_int", BinaryOp::Lt) => "query_param_int_lt",
        ("query_param_int", BinaryOp::Le) => "query_param_int_le",
        ("query_param_int", BinaryOp::Gt) => "query_param_int_gt",
        ("query_param_int", BinaryOp::Ge) => "query_param_int_ge",
        _ => return None,
    };
    Some(kind.to_string())
}

fn condition_kind_for_float_operand(op: BinaryOp, operand_kind: &str) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field_float", BinaryOp::Eq) => "request_body_field_float_eq",
        ("request_body_field_float", BinaryOp::Ne) => "request_body_field_float_ne",
        ("request_body_field_float", BinaryOp::Lt) => "request_body_field_float_lt",
        ("request_body_field_float", BinaryOp::Le) => "request_body_field_float_le",
        ("request_body_field_float", BinaryOp::Gt) => "request_body_field_float_gt",
        ("request_body_field_float", BinaryOp::Ge) => "request_body_field_float_ge",
        ("route_param_float", BinaryOp::Eq) => "route_param_float_eq",
        ("route_param_float", BinaryOp::Ne) => "route_param_float_ne",
        ("route_param_float", BinaryOp::Lt) => "route_param_float_lt",
        ("route_param_float", BinaryOp::Le) => "route_param_float_le",
        ("route_param_float", BinaryOp::Gt) => "route_param_float_gt",
        ("route_param_float", BinaryOp::Ge) => "route_param_float_ge",
        ("query_param_float", BinaryOp::Eq) => "query_param_float_eq",
        ("query_param_float", BinaryOp::Ne) => "query_param_float_ne",
        ("query_param_float", BinaryOp::Lt) => "query_param_float_lt",
        ("query_param_float", BinaryOp::Le) => "query_param_float_le",
        ("query_param_float", BinaryOp::Gt) => "query_param_float_gt",
        ("query_param_float", BinaryOp::Ge) => "query_param_float_ge",
        _ => return None,
    };
    Some(kind.to_string())
}

fn condition_kind_for_bool_operand(op: BinaryOp, operand_kind: &str) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field_bool", BinaryOp::Eq) => "request_body_field_bool_eq",
        ("request_body_field_bool", BinaryOp::Ne) => "request_body_field_bool_ne",
        ("request_body_field_bool", BinaryOp::And) => "request_body_field_bool_and",
        ("request_body_field_bool", BinaryOp::Or) => "request_body_field_bool_or",
        ("route_param_bool", BinaryOp::Eq) => "route_param_bool_eq",
        ("route_param_bool", BinaryOp::Ne) => "route_param_bool_ne",
        ("route_param_bool", BinaryOp::And) => "route_param_bool_and",
        ("route_param_bool", BinaryOp::Or) => "route_param_bool_or",
        ("query_param_bool", BinaryOp::Eq) => "query_param_bool_eq",
        ("query_param_bool", BinaryOp::Ne) => "query_param_bool_ne",
        ("query_param_bool", BinaryOp::And) => "query_param_bool_and",
        ("query_param_bool", BinaryOp::Or) => "query_param_bool_or",
        _ => return None,
    };
    Some(kind.to_string())
}

fn condition_kind_for_negated_bool_operand(op: BinaryOp, operand_kind: &str) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field_bool", BinaryOp::And) => "request_body_field_bool_not_and",
        ("request_body_field_bool", BinaryOp::Or) => "request_body_field_bool_not_or",
        ("route_param_bool", BinaryOp::And) => "route_param_bool_not_and",
        ("route_param_bool", BinaryOp::Or) => "route_param_bool_not_or",
        ("query_param_bool", BinaryOp::And) => "query_param_bool_not_and",
        ("query_param_bool", BinaryOp::Or) => "query_param_bool_not_or",
        _ => return None,
    };
    Some(kind.to_string())
}

fn condition_kind_for_right_negated_bool_operand(
    op: BinaryOp,
    operand_kind: &str,
) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field_bool", BinaryOp::And) => "request_body_field_bool_and_not",
        ("request_body_field_bool", BinaryOp::Or) => "request_body_field_bool_or_not",
        ("route_param_bool", BinaryOp::And) => "route_param_bool_and_not",
        ("route_param_bool", BinaryOp::Or) => "route_param_bool_or_not",
        ("query_param_bool", BinaryOp::And) => "query_param_bool_and_not",
        ("query_param_bool", BinaryOp::Or) => "query_param_bool_or_not",
        _ => return None,
    };
    Some(kind.to_string())
}

fn guarded_route_response_artifacts(handler: &HirBlock) -> Option<Vec<ServerResponseArtifact>> {
    if let Some(responses) = if_else_route_response_artifacts(handler) {
        return Some(responses);
    }
    if handler.stmts.len() < 2 {
        return None;
    }
    let mut out = Vec::with_capacity(handler.stmts.len());
    for (index, stmt) in handler.stmts.iter().enumerate() {
        if index + 1 == handler.stmts.len() {
            let (respond, status, payload) = response_expr_from_stmt(stmt)?;
            out.push(server_response_artifact(respond, status, payload, None));
            continue;
        }
        collect_guarded_response_artifacts_from_stmt(stmt, &mut out)?;
    }
    Some(out)
}

fn collect_guarded_response_artifacts_from_stmt(
    stmt: &HirStmt,
    out: &mut Vec<ServerResponseArtifact>,
) -> Option<()> {
    let HirStmt::Expr(expr) = stmt else {
        return None;
    };
    collect_guarded_response_artifacts_from_if(expr, out)
}

fn collect_guarded_response_artifacts_from_if(
    expr: &HirExpr,
    out: &mut Vec<ServerResponseArtifact>,
) -> Option<()> {
    let HirExprKind::If {
        cond,
        then,
        else_branch,
    } = &expr.kind
    else {
        return None;
    };
    if then.stmts.len() != 1 {
        return None;
    }
    let condition = native_response_condition(cond)?;
    let (respond, status, payload) = response_expr_from_stmt(&then.stmts[0])?;
    out.push(server_response_artifact(
        respond,
        status,
        payload,
        Some(condition),
    ));
    if let Some(else_branch) = else_branch {
        collect_guarded_else_if_response_artifacts(else_branch, out)?;
    }
    Some(())
}

fn collect_guarded_else_if_response_artifacts(
    expr: &HirExpr,
    out: &mut Vec<ServerResponseArtifact>,
) -> Option<()> {
    match &expr.kind {
        HirExprKind::If { .. } => collect_guarded_response_artifacts_from_if(expr, out),
        HirExprKind::Block(block) => {
            let [HirStmt::Expr(expr)] = block.stmts.as_slice() else {
                return None;
            };
            collect_guarded_else_if_response_artifacts(expr, out)
        }
        HirExprKind::Paren(expr) => collect_guarded_else_if_response_artifacts(expr, out),
        _ => None,
    }
}

fn if_else_route_response_artifacts(handler: &HirBlock) -> Option<Vec<ServerResponseArtifact>> {
    let [HirStmt::Expr(expr)] = handler.stmts.as_slice() else {
        return None;
    };
    let mut out = Vec::new();
    collect_if_else_response_artifacts(expr, &mut out)?;
    (out.len() >= 2).then_some(out)
}

fn collect_if_else_response_artifacts(
    expr: &HirExpr,
    out: &mut Vec<ServerResponseArtifact>,
) -> Option<()> {
    let HirExprKind::If {
        cond,
        then,
        else_branch: Some(else_branch),
    } = &expr.kind
    else {
        return None;
    };
    if then.stmts.len() != 1 {
        return None;
    }
    let condition = native_response_condition(cond)?;
    let (then_respond, then_status, then_payload) = response_expr_from_stmt(&then.stmts[0])?;
    out.push(server_response_artifact(
        then_respond,
        then_status,
        then_payload,
        Some(condition),
    ));
    collect_else_response_artifacts(else_branch, out)
}

fn collect_else_response_artifacts(
    expr: &HirExpr,
    out: &mut Vec<ServerResponseArtifact>,
) -> Option<()> {
    match &expr.kind {
        HirExprKind::If { .. } => collect_if_else_response_artifacts(expr, out),
        HirExprKind::Block(block) => {
            let [HirStmt::Expr(expr)] = block.stmts.as_slice() else {
                return None;
            };
            if matches!(expr.kind, HirExprKind::If { .. }) {
                return collect_if_else_response_artifacts(expr, out);
            }
            let (respond, status, payload) = response_expr(expr)?;
            out.push(server_response_artifact(respond, status, payload, None));
            Some(())
        }
        HirExprKind::Paren(expr) => collect_else_response_artifacts(expr, out),
        _ => {
            let (respond, status, payload) = response_expr_from_else_branch(expr)?;
            out.push(server_response_artifact(respond, status, payload, None));
            Some(())
        }
    }
}

fn response_expr_from_stmt(stmt: &HirStmt) -> Option<(&HirExpr, &HirExpr, &HirExpr)> {
    let HirStmt::Expr(expr) = stmt else {
        return None;
    };
    response_expr(expr)
}

fn response_expr_from_else_branch(expr: &HirExpr) -> Option<(&HirExpr, &HirExpr, &HirExpr)> {
    match &expr.kind {
        HirExprKind::Block(block) => {
            let [stmt] = block.stmts.as_slice() else {
                return None;
            };
            response_expr_from_stmt(stmt)
        }
        HirExprKind::Paren(expr) => response_expr_from_else_branch(expr),
        _ => response_expr(expr),
    }
}

fn response_expr(expr: &HirExpr) -> Option<(&HirExpr, &HirExpr, &HirExpr)> {
    match &expr.kind {
        HirExprKind::Respond { status, payload } => Some((expr, status, payload)),
        HirExprKind::Paren(expr) => response_expr(expr),
        _ => None,
    }
}

fn write_json_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(ch));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

pub fn json_escaped(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(ch));
            }
            ch => out.push(ch),
        }
    }
    out
}

/// Verify that a server runtime artifact is internally consistent.
///
/// This checks the source bundle hashes and route descriptor shape. It does not
/// replace type checking; it validates that an emitted artifact was not
/// accidentally corrupted before a runner hydrates it.
///
/// # Errors
///
/// Returns all validation failures when source hashes do not match, source
/// bundle files are missing paths, or route descriptors are incomplete.
pub fn verify_server_runtime_artifact(artifact: &ServerRuntimeArtifact) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if artifact.source_bundle.files.is_empty() {
        errors.push("source bundle is empty".to_string());
    }
    for file in &artifact.source_bundle.files {
        if file.path.is_empty() {
            errors.push("source bundle contains file with empty path".to_string());
        }
        let actual = content_hash(&file.source);
        if file.content_hash != actual {
            errors.push(format!(
                "content hash mismatch for {}: expected {}, got {}",
                file.path, file.content_hash, actual
            ));
        }
    }
    for route in &artifact.routes {
        if route.method.is_empty() {
            errors.push("route method is empty".to_string());
        }
        if route.path.is_empty() {
            errors.push("route path is empty".to_string());
        }
        if route.origin_id.is_empty() {
            errors.push(format!(
                "route {} {} has empty origin id",
                route.method, route.path
            ));
        }
        for policy in &route.policies {
            verify_route_policy_artifact(route, policy, &mut errors);
        }
    }
    if let Some(listen) = &artifact.listen {
        if listen.origin_id.is_empty() {
            errors.push("listen origin_id is empty".to_string());
        }
        if listen.name.is_empty() {
            errors.push("listen name is empty".to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn verify_route_policy_artifact(
    route: &ServerRouteArtifact,
    policy: &ServerRoutePolicyArtifact,
    errors: &mut Vec<String>,
) {
    if policy.kind.is_empty() {
        errors.push(format!(
            "route {} {} has policy with empty kind",
            route.method, route.path
        ));
        return;
    }
    match policy.kind.as_str() {
        "csrf" => {
            if policy.origin_id.as_deref().is_none_or(str::is_empty) {
                errors.push(format!(
                    "route {} {} csrf policy has empty origin id",
                    route.method, route.path
                ));
            }
            if policy.exempt == Some(true) {
                if policy.required == Some(true) {
                    errors.push(format!(
                        "route {} {} csrf exempt policy must not be required",
                        route.method, route.path
                    ));
                }
            } else if policy.required != Some(true) {
                errors.push(format!(
                    "route {} {} csrf policy must be required",
                    route.method, route.path
                ));
            }
            if policy.key.is_some()
                || policy.role.is_some()
                || policy.limit.is_some()
                || policy.window_seconds.is_some()
            {
                errors.push(format!(
                    "route {} {} csrf policy must not set rate-limit/auth fields",
                    route.method, route.path
                ));
            }
        }
        "session" | "auth" => {
            if policy.origin_id.as_deref().is_none_or(str::is_empty) {
                errors.push(format!(
                    "route {} {} {} policy has empty origin id",
                    route.method, route.path, policy.kind
                ));
            }
            if policy.required != Some(true) {
                errors.push(format!(
                    "route {} {} {} policy must be required",
                    route.method, route.path, policy.kind
                ));
            }
            if policy.key.is_some()
                || policy.exempt.is_some()
                || policy.limit.is_some()
                || policy.window_seconds.is_some()
            {
                errors.push(format!(
                    "route {} {} {} policy must not set rate-limit fields",
                    route.method, route.path, policy.kind
                ));
            }
        }
        "rate_limit" => {
            if (policy.key.is_some() || policy.exempt.is_some())
                && policy.origin_id.as_deref().is_none_or(str::is_empty)
            {
                errors.push(format!(
                    "route {} {} explicit rate_limit policy has empty origin id",
                    route.method, route.path
                ));
            }
            if policy.exempt == Some(true) {
                if policy.limit.is_some() || policy.window_seconds.is_some() {
                    errors.push(format!(
                        "route {} {} rate_limit exempt policy must not set limit/window_seconds",
                        route.method, route.path
                    ));
                }
                return;
            }
            if policy.limit.unwrap_or_default() == 0 {
                errors.push(format!(
                    "route {} {} rate_limit policy must have a positive limit",
                    route.method, route.path
                ));
            }
            if policy.window_seconds.unwrap_or_default() == 0 {
                errors.push(format!(
                    "route {} {} rate_limit policy must have a positive window_seconds",
                    route.method, route.path
                ));
            }
            if policy.required.is_some() || policy.role.is_some() {
                errors.push(format!(
                    "route {} {} rate_limit policy must not set auth fields",
                    route.method, route.path
                ));
            }
        }
        _ => errors.push(format!(
            "route {} {} has unknown policy kind {}",
            route.method, route.path, policy.kind
        )),
    }
}

/// Verify that a build source bundle artifact is internally consistent.
///
/// # Errors
///
/// Returns all validation failures when source hashes do not match or source
/// bundle files are missing paths.
pub fn verify_source_bundle_artifact(artifact: &SourceBundleArtifact) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if artifact.schema_version != SOURCE_BUNDLE_ARTIFACT_VERSION {
        errors.push(format!(
            "unsupported source bundle schema_version {}",
            artifact.schema_version
        ));
    }
    if artifact.entry.trim().is_empty() {
        errors.push("source bundle entry is empty".to_string());
    }
    if artifact.files.is_empty() {
        errors.push("source bundle is empty".to_string());
    }
    for file in &artifact.files {
        if file.path.trim().is_empty() {
            errors.push("source bundle contains file with empty path".to_string());
        }
        let actual = content_hash(&file.source);
        if file.content_hash != actual {
            errors.push(format!(
                "content hash mismatch for {}: expected {}, got {}",
                file.path, file.content_hash, actual
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn route_artifact(
    entry: &OriginEntry,
    origin_map: &OriginMap,
    responses_by_route: &HashMap<String, Vec<ServerResponseArtifact>>,
    policies_by_route: &HashMap<String, Vec<ServerRoutePolicyArtifact>>,
) -> Option<ServerRouteArtifact> {
    let (method, path) = entry.name.split_once(' ')?;
    Some(ServerRouteArtifact {
        method: method.to_string(),
        path: path.to_string(),
        origin_id: entry.id.clone(),
        response_origin_ids: route_response_origin_ids(&entry.id, origin_map),
        responses: responses_by_route
            .get(&entry.id)
            .cloned()
            .unwrap_or_default(),
        policies: policies_by_route
            .get(&entry.id)
            .cloned()
            .unwrap_or_default(),
    })
}

pub fn route_policy_artifacts(
    program: &HirProgram,
) -> HashMap<String, Vec<ServerRoutePolicyArtifact>> {
    let mut out = HashMap::new();
    for stmt in &program.items {
        collect_stmt_policy_artifacts(stmt, None, &mut out);
    }
    out
}

fn collect_stmt_policy_artifacts(
    stmt: &HirStmt,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerRoutePolicyArtifact>>,
) {
    match stmt {
        HirStmt::Let(stmt) => collect_expr_policy_artifacts(&stmt.init, route_origin_id, out),
        HirStmt::Const(stmt) => collect_expr_policy_artifacts(&stmt.init, route_origin_id, out),
        HirStmt::Function(stmt) => {
            collect_function_body_policy_artifacts(&stmt.body, route_origin_id, out);
        }
        HirStmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_policy_artifacts(value, route_origin_id, out);
            }
        }
        HirStmt::Expr(expr) => collect_expr_policy_artifacts(expr, route_origin_id, out),
        HirStmt::Struct(_) | HirStmt::Enum(_) | HirStmt::TypeAlias(_) | HirStmt::Import(_) => {}
    }
}

fn collect_block_policy_artifacts(
    block: &HirBlock,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerRoutePolicyArtifact>>,
) {
    for stmt in &block.stmts {
        collect_stmt_policy_artifacts(stmt, route_origin_id, out);
    }
}

fn collect_function_body_policy_artifacts(
    body: &HirFunctionBody,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerRoutePolicyArtifact>>,
) {
    match body {
        HirFunctionBody::Block(block) => {
            collect_block_policy_artifacts(block, route_origin_id, out);
        }
        HirFunctionBody::Expr(expr) => collect_expr_policy_artifacts(expr, route_origin_id, out),
    }
}

#[allow(clippy::too_many_lines)]
fn collect_expr_policy_artifacts(
    expr: &HirExpr,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerRoutePolicyArtifact>>,
) {
    match &expr.kind {
        HirExprKind::Route {
            method,
            path,
            handler,
            ..
        } => {
            let name = format!("{method} {path}");
            let route_origin = origin_id("route", &name, expr.span);
            collect_block_policy_artifacts(handler, Some(&route_origin), out);
            if !out
                .get(&route_origin)
                .is_some_and(|policies| policies.iter().any(|policy| policy.kind == "rate_limit"))
            {
                let defaults = default_route_policy_artifacts(method, path);
                if !defaults.is_empty() {
                    let policies = out.entry(route_origin).or_default();
                    for policy in defaults.into_iter().rev() {
                        policies.insert(0, policy);
                    }
                }
            }
        }
        HirExprKind::Domain { name, args, .. } => {
            if let Some(route_origin_id) = route_origin_id {
                if let Some(policy) = route_policy_artifact_from_domain(expr, name, args) {
                    out.entry(route_origin_id.to_string())
                        .or_default()
                        .push(policy);
                }
            }
            for arg in args {
                collect_expr_policy_artifacts(arg, route_origin_id, out);
            }
        }
        HirExprKind::Server {
            listen,
            routes,
            body_stmts,
        } => {
            if let Some(listen) = listen {
                collect_expr_policy_artifacts(listen, route_origin_id, out);
            }
            for route in routes {
                collect_expr_policy_artifacts(route, route_origin_id, out);
            }
            for stmt in body_stmts {
                collect_stmt_policy_artifacts(stmt, route_origin_id, out);
            }
        }
        HirExprKind::Out(inner)
        | HirExprKind::Unary { expr: inner, .. }
        | HirExprKind::Paren(inner)
        | HirExprKind::Throw(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Cast { expr: inner, .. } => {
            collect_expr_policy_artifacts(inner, route_origin_id, out);
        }
        HirExprKind::Respond { status, payload } => {
            collect_expr_policy_artifacts(status, route_origin_id, out);
            collect_expr_policy_artifacts(payload, route_origin_id, out);
        }
        HirExprKind::Html(block) | HirExprKind::Block(block) => {
            collect_block_policy_artifacts(block, route_origin_id, out);
        }
        HirExprKind::Call { callee, args } => {
            collect_expr_policy_artifacts(callee, route_origin_id, out);
            for arg in args {
                collect_expr_policy_artifacts(arg, route_origin_id, out);
            }
        }
        HirExprKind::String(segments) => {
            for segment in segments {
                if let HirStringSegment::Interp(expr) = segment {
                    collect_expr_policy_artifacts(expr, route_origin_id, out);
                }
            }
        }
        HirExprKind::Binary { lhs, rhs, .. } => {
            collect_expr_policy_artifacts(lhs, route_origin_id, out);
            collect_expr_policy_artifacts(rhs, route_origin_id, out);
        }
        HirExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            collect_expr_policy_artifacts(cond, route_origin_id, out);
            collect_block_policy_artifacts(then, route_origin_id, out);
            if let Some(expr) = else_branch {
                collect_expr_policy_artifacts(expr, route_origin_id, out);
            }
        }
        HirExprKind::When { scrutinee, arms } => {
            collect_expr_policy_artifacts(scrutinee, route_origin_id, out);
            for arm in arms {
                collect_pattern_policy_artifacts(&arm.pattern, route_origin_id, out);
                collect_expr_policy_artifacts(&arm.body, route_origin_id, out);
            }
        }
        HirExprKind::Assign { value, .. } => {
            collect_expr_policy_artifacts(value, route_origin_id, out);
        }
        HirExprKind::AssignField { object, value, .. } => {
            collect_expr_policy_artifacts(object, route_origin_id, out);
            collect_expr_policy_artifacts(value, route_origin_id, out);
        }
        HirExprKind::AssignIndex {
            object,
            index,
            value,
        } => {
            collect_expr_policy_artifacts(object, route_origin_id, out);
            collect_expr_policy_artifacts(index, route_origin_id, out);
            collect_expr_policy_artifacts(value, route_origin_id, out);
        }
        HirExprKind::For { iter, body, .. } => {
            collect_expr_policy_artifacts(iter, route_origin_id, out);
            collect_block_policy_artifacts(body, route_origin_id, out);
        }
        HirExprKind::While { cond, body } => {
            collect_expr_policy_artifacts(cond, route_origin_id, out);
            collect_block_policy_artifacts(body, route_origin_id, out);
        }
        HirExprKind::Range { start, end, .. } => {
            collect_expr_policy_artifacts(start, route_origin_id, out);
            collect_expr_policy_artifacts(end, route_origin_id, out);
        }
        HirExprKind::Array(items) | HirExprKind::Tuple(items) => {
            for item in items {
                collect_expr_policy_artifacts(item, route_origin_id, out);
            }
        }
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            for field in fields {
                collect_expr_policy_artifacts(&field.value, route_origin_id, out);
            }
        }
        HirExprKind::Index { target, index } => {
            collect_expr_policy_artifacts(target, route_origin_id, out);
            collect_expr_policy_artifacts(index, route_origin_id, out);
        }
        HirExprKind::Slice { target, start, end } => {
            collect_expr_policy_artifacts(target, route_origin_id, out);
            if let Some(start) = start {
                collect_expr_policy_artifacts(start, route_origin_id, out);
            }
            if let Some(end) = end {
                collect_expr_policy_artifacts(end, route_origin_id, out);
            }
        }
        HirExprKind::Field { target, .. } | HirExprKind::OptionalField { target, .. } => {
            collect_expr_policy_artifacts(target, route_origin_id, out);
        }
        HirExprKind::Lambda { body, .. } => {
            collect_function_body_policy_artifacts(body, route_origin_id, out);
        }
        HirExprKind::Try { try_block, catch } => {
            collect_block_policy_artifacts(try_block, route_origin_id, out);
            if let Some(catch) = catch {
                collect_block_policy_artifacts(&catch.body, route_origin_id, out);
            }
        }
        HirExprKind::Integer(_)
        | HirExprKind::Float(_)
        | HirExprKind::Regex { .. }
        | HirExprKind::True
        | HirExprKind::False
        | HirExprKind::Void
        | HirExprKind::TypeName(_)
        | HirExprKind::Ident(_)
        | HirExprKind::Break
        | HirExprKind::Continue => {}
    }
}

fn collect_pattern_policy_artifacts(
    pattern: &HirPattern,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerRoutePolicyArtifact>>,
) {
    match pattern {
        HirPattern::Literal(expr)
        | HirPattern::Guard(expr)
        | HirPattern::Not(expr)
        | HirPattern::Contains(expr) => collect_expr_policy_artifacts(expr, route_origin_id, out),
        HirPattern::Range { start, end, .. } => {
            collect_expr_policy_artifacts(start, route_origin_id, out);
            collect_expr_policy_artifacts(end, route_origin_id, out);
        }
        HirPattern::Wildcard => {}
    }
}

fn route_policy_artifact_from_domain(
    expr: &HirExpr,
    name: &str,
    args: &[HirExpr],
) -> Option<ServerRoutePolicyArtifact> {
    let origin_id = Some(origin_id("domain", name, expr.span));
    match name {
        "csrf" if args.is_empty() => Some(ServerRoutePolicyArtifact {
            kind: "csrf".to_string(),
            origin_id,
            required: Some(true),
            role: None,
            key: None,
            exempt: None,
            limit: None,
            window_seconds: None,
        }),
        "csrf" if args.iter().any(is_exempt_policy_arg) => Some(ServerRoutePolicyArtifact {
            kind: "csrf".to_string(),
            origin_id,
            required: Some(false),
            role: None,
            key: None,
            exempt: Some(true),
            limit: None,
            window_seconds: None,
        }),
        "session" if args.iter().any(is_required_policy_arg) => Some(ServerRoutePolicyArtifact {
            kind: "session".to_string(),
            origin_id,
            required: Some(true),
            role: None,
            key: None,
            exempt: None,
            limit: None,
            window_seconds: None,
        }),
        "Auth" => {
            let (required, role) = auth_policy_args(args);
            if required || role.is_some() {
                Some(ServerRoutePolicyArtifact {
                    kind: "auth".to_string(),
                    origin_id,
                    required: Some(true),
                    role,
                    key: None,
                    exempt: None,
                    limit: None,
                    window_seconds: None,
                })
            } else {
                None
            }
        }
        "rateLimit" => rate_limit_policy_artifact(origin_id, args),
        _ => None,
    }
}

fn rate_limit_policy_artifact(
    origin_id: Option<String>,
    args: &[HirExpr],
) -> Option<ServerRoutePolicyArtifact> {
    let mut key = None;
    let mut exempt = false;
    let mut limit = None;
    let mut window_seconds = None;
    for arg in args {
        match &arg.kind {
            HirExprKind::Ident(ident) if ident.name == "exempt" => {
                exempt = true;
            }
            HirExprKind::Assign { target, value } if target.name == "exempt" => {
                exempt = static_bool(value).unwrap_or(false);
            }
            HirExprKind::Assign { target, value }
                if matches!(target.name.as_str(), "limit" | "max") =>
            {
                limit = static_positive_u32(value);
            }
            HirExprKind::Assign { target, value } if target.name == "window" => {
                window_seconds = static_duration_seconds(value);
            }
            HirExprKind::Assign { target, value } if target.name == "key" => {
                key = static_rate_limit_key(value);
            }
            _ => {}
        }
    }
    if exempt {
        return Some(ServerRoutePolicyArtifact {
            kind: "rate_limit".to_string(),
            origin_id,
            required: None,
            role: None,
            key,
            exempt: Some(true),
            limit: None,
            window_seconds: None,
        });
    }
    Some(ServerRoutePolicyArtifact {
        kind: "rate_limit".to_string(),
        origin_id,
        required: None,
        role: None,
        key,
        exempt: None,
        limit,
        window_seconds,
    })
}

fn default_route_policy_artifacts(method: &str, path: &str) -> Vec<ServerRoutePolicyArtifact> {
    let Some((limit, window_seconds)) = default_route_rate_limit(method, path) else {
        return Vec::new();
    };
    vec![ServerRoutePolicyArtifact {
        kind: "rate_limit".to_string(),
        origin_id: None,
        required: None,
        role: None,
        key: None,
        exempt: None,
        limit: Some(limit),
        window_seconds: Some(window_seconds),
    }]
}

fn static_positive_u32(expr: &HirExpr) -> Option<u32> {
    static_integer(expr).and_then(|value| u32::try_from(value).ok().filter(|value| *value > 0))
}

fn static_duration_seconds(expr: &HirExpr) -> Option<u32> {
    static_positive_u32(expr).or_else(|| {
        static_string_literal(expr).and_then(|value| parse_duration_seconds_literal(&value))
    })
}

fn parse_duration_seconds_literal(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let digit_len = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '_')
        .map(char::len_utf8)
        .sum::<usize>();
    let (amount, unit) = trimmed.split_at(digit_len);
    let amount = amount.replace('_', "").parse::<u32>().ok()?;
    if amount == 0 {
        return None;
    }
    let multiplier = match unit.trim() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

fn static_rate_limit_key(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::String(_) => static_string_literal(expr),
        HirExprKind::Ident(ident) => Some(ident.name.clone()),
        HirExprKind::Domain { name, args, .. } if args.is_empty() => Some(format!("@{name}")),
        HirExprKind::Field { target, field, .. } => {
            static_rate_limit_key(target).map(|target| format!("{target}.{field}"))
        }
        HirExprKind::OptionalField { target, field, .. } => {
            static_rate_limit_key(target).map(|target| format!("{target}?.{field}"))
        }
        HirExprKind::Paren(expr) => static_rate_limit_key(expr),
        _ => None,
    }
}

fn default_route_rate_limit(method: &str, path: &str) -> Option<(u32, u32)> {
    match (method, path) {
        ("POST", "/members/login" | "/checkout") => Some((10, 60)),
        ("POST", "/webhooks/stripe") => Some((60, 60)),
        _ => None,
    }
}

fn is_required_policy_arg(arg: &HirExpr) -> bool {
    matches!(&arg.kind, HirExprKind::Ident(ident) if ident.name == "required")
        || matches!(&arg.kind, HirExprKind::Assign { target, value }
            if target.name == "required" && matches!(&value.kind, HirExprKind::True))
}

fn is_exempt_policy_arg(arg: &HirExpr) -> bool {
    matches!(&arg.kind, HirExprKind::Ident(ident) if ident.name == "exempt")
        || matches!(&arg.kind, HirExprKind::Assign { target, value }
            if target.name == "exempt" && static_bool(value) == Some(true))
}

fn auth_policy_args(args: &[HirExpr]) -> (bool, Option<String>) {
    let mut required = false;
    let mut role = None;
    for arg in args {
        match &arg.kind {
            HirExprKind::Ident(ident) if ident.name == "required" => {
                required = true;
            }
            HirExprKind::Assign { target, value } if target.name == "required" => {
                required = matches!(&value.kind, HirExprKind::True);
            }
            HirExprKind::Assign { target, value } if target.name == "role" => {
                role = static_string_literal(value);
            }
            _ => {}
        }
    }
    if role.is_some() {
        required = true;
    }
    (required, role)
}

fn static_string_literal(expr: &HirExpr) -> Option<String> {
    let HirExprKind::String(segments) = &expr.kind else {
        return None;
    };
    let [HirStringSegment::Str(value)] = segments.as_slice() else {
        return None;
    };
    Some(value.clone())
}

pub fn route_response_artifacts(
    program: &HirProgram,
) -> HashMap<String, Vec<ServerResponseArtifact>> {
    let mut out = HashMap::new();
    for stmt in &program.items {
        collect_stmt_response_artifacts(stmt, None, &mut out);
    }
    out
}

fn collect_stmt_response_artifacts(
    stmt: &HirStmt,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    match stmt {
        HirStmt::Let(stmt) => collect_expr_response_artifacts(&stmt.init, route_origin_id, out),
        HirStmt::Const(stmt) => collect_expr_response_artifacts(&stmt.init, route_origin_id, out),
        HirStmt::Function(stmt) => {
            collect_function_body_response_artifacts(&stmt.body, route_origin_id, out);
        }
        HirStmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_response_artifacts(value, route_origin_id, out);
            }
        }
        HirStmt::Expr(expr) => collect_expr_response_artifacts(expr, route_origin_id, out),
        HirStmt::Struct(_) | HirStmt::Enum(_) | HirStmt::TypeAlias(_) | HirStmt::Import(_) => {}
    }
}

fn collect_block_response_artifacts(
    block: &HirBlock,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    for stmt in &block.stmts {
        collect_stmt_response_artifacts(stmt, route_origin_id, out);
    }
}

fn collect_function_body_response_artifacts(
    body: &HirFunctionBody,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    match body {
        HirFunctionBody::Block(block) => {
            collect_block_response_artifacts(block, route_origin_id, out);
        }
        HirFunctionBody::Expr(expr) => collect_expr_response_artifacts(expr, route_origin_id, out),
    }
}

#[allow(clippy::too_many_lines)]
fn collect_expr_response_artifacts(
    expr: &HirExpr,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    match &expr.kind {
        HirExprKind::Route {
            method,
            path,
            handler,
            ..
        } => {
            let name = format!("{method} {path}");
            let route_origin = origin_id("route", &name, expr.span);
            if let Some(responses) = guarded_route_response_artifacts(handler) {
                out.entry(route_origin).or_default().extend(responses);
            } else {
                collect_block_response_artifacts(handler, Some(&route_origin), out);
            }
        }
        HirExprKind::Respond { status, payload } => {
            if let Some(route_origin_id) = route_origin_id {
                out.entry(route_origin_id.to_string())
                    .or_default()
                    .push(server_response_artifact(expr, status, payload, None));
            }
            collect_expr_response_artifacts(status, route_origin_id, out);
            collect_expr_response_artifacts(payload, route_origin_id, out);
        }
        HirExprKind::Server {
            listen,
            routes,
            body_stmts,
        } => {
            if let Some(listen) = listen {
                collect_expr_response_artifacts(listen, route_origin_id, out);
            }
            for route in routes {
                collect_expr_response_artifacts(route, route_origin_id, out);
            }
            for stmt in body_stmts {
                collect_stmt_response_artifacts(stmt, route_origin_id, out);
            }
        }
        HirExprKind::Out(inner)
        | HirExprKind::Unary { expr: inner, .. }
        | HirExprKind::Paren(inner)
        | HirExprKind::Throw(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Cast { expr: inner, .. } => {
            collect_expr_response_artifacts(inner, route_origin_id, out);
        }
        HirExprKind::Html(block) | HirExprKind::Block(block) => {
            collect_block_response_artifacts(block, route_origin_id, out);
        }
        HirExprKind::Domain { args, .. } => {
            for arg in args {
                collect_expr_response_artifacts(arg, route_origin_id, out);
            }
        }
        HirExprKind::Call { callee, args } => {
            collect_expr_response_artifacts(callee, route_origin_id, out);
            for arg in args {
                collect_expr_response_artifacts(arg, route_origin_id, out);
            }
        }
        HirExprKind::String(segments) => {
            for segment in segments {
                if let HirStringSegment::Interp(expr) = segment {
                    collect_expr_response_artifacts(expr, route_origin_id, out);
                }
            }
        }
        HirExprKind::Binary { lhs, rhs, .. } => {
            collect_expr_response_artifacts(lhs, route_origin_id, out);
            collect_expr_response_artifacts(rhs, route_origin_id, out);
        }
        HirExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            collect_expr_response_artifacts(cond, route_origin_id, out);
            collect_block_response_artifacts(then, route_origin_id, out);
            if let Some(expr) = else_branch {
                collect_expr_response_artifacts(expr, route_origin_id, out);
            }
        }
        HirExprKind::When { scrutinee, arms } => {
            collect_expr_response_artifacts(scrutinee, route_origin_id, out);
            for arm in arms {
                collect_pattern_response_artifacts(&arm.pattern, route_origin_id, out);
                collect_expr_response_artifacts(&arm.body, route_origin_id, out);
            }
        }
        HirExprKind::Assign { value, .. } => {
            collect_expr_response_artifacts(value, route_origin_id, out);
        }
        HirExprKind::AssignField { object, value, .. } => {
            collect_expr_response_artifacts(object, route_origin_id, out);
            collect_expr_response_artifacts(value, route_origin_id, out);
        }
        HirExprKind::AssignIndex {
            object,
            index,
            value,
        } => {
            collect_expr_response_artifacts(object, route_origin_id, out);
            collect_expr_response_artifacts(index, route_origin_id, out);
            collect_expr_response_artifacts(value, route_origin_id, out);
        }
        HirExprKind::For { iter, body, .. } => {
            collect_expr_response_artifacts(iter, route_origin_id, out);
            collect_block_response_artifacts(body, route_origin_id, out);
        }
        HirExprKind::While { cond, body } => {
            collect_expr_response_artifacts(cond, route_origin_id, out);
            collect_block_response_artifacts(body, route_origin_id, out);
        }
        HirExprKind::Range { start, end, .. } => {
            collect_expr_response_artifacts(start, route_origin_id, out);
            collect_expr_response_artifacts(end, route_origin_id, out);
        }
        HirExprKind::Array(items) | HirExprKind::Tuple(items) => {
            for item in items {
                collect_expr_response_artifacts(item, route_origin_id, out);
            }
        }
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            for field in fields {
                collect_expr_response_artifacts(&field.value, route_origin_id, out);
            }
        }
        HirExprKind::Index { target, index } => {
            collect_expr_response_artifacts(target, route_origin_id, out);
            collect_expr_response_artifacts(index, route_origin_id, out);
        }
        HirExprKind::Slice { target, start, end } => {
            collect_expr_response_artifacts(target, route_origin_id, out);
            if let Some(start) = start {
                collect_expr_response_artifacts(start, route_origin_id, out);
            }
            if let Some(end) = end {
                collect_expr_response_artifacts(end, route_origin_id, out);
            }
        }
        HirExprKind::Field { target, .. } | HirExprKind::OptionalField { target, .. } => {
            collect_expr_response_artifacts(target, route_origin_id, out);
        }
        HirExprKind::Lambda { body, .. } => {
            collect_function_body_response_artifacts(body, route_origin_id, out);
        }
        HirExprKind::Try { try_block, catch } => {
            collect_block_response_artifacts(try_block, route_origin_id, out);
            if let Some(catch) = catch {
                collect_block_response_artifacts(&catch.body, route_origin_id, out);
            }
        }
        HirExprKind::Integer(_)
        | HirExprKind::Float(_)
        | HirExprKind::Regex { .. }
        | HirExprKind::True
        | HirExprKind::False
        | HirExprKind::Void
        | HirExprKind::TypeName(_)
        | HirExprKind::Ident(_)
        | HirExprKind::Break
        | HirExprKind::Continue => {}
    }
}

fn collect_pattern_response_artifacts(
    pattern: &HirPattern,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    match pattern {
        HirPattern::Literal(expr)
        | HirPattern::Guard(expr)
        | HirPattern::Not(expr)
        | HirPattern::Contains(expr) => collect_expr_response_artifacts(expr, route_origin_id, out),
        HirPattern::Range { start, end, .. } => {
            collect_expr_response_artifacts(start, route_origin_id, out);
            collect_expr_response_artifacts(end, route_origin_id, out);
        }
        HirPattern::Wildcard => {}
    }
}

fn route_response_origin_ids(route_origin_id: &str, origin_map: &OriginMap) -> Vec<String> {
    let mut response_origin_ids = Vec::new();
    for edge in origin_map
        .edges
        .iter()
        .filter(|edge| edge.from == route_origin_id && edge.kind == "contains")
    {
        let Some(entry) = origin_map
            .entries
            .iter()
            .find(|entry| entry.id == edge.to && entry.kind == "domain" && entry.name == "respond")
        else {
            continue;
        };
        if !response_origin_ids.contains(&entry.id) {
            response_origin_ids.push(entry.id.clone());
        }
    }
    response_origin_ids
}

pub fn listen_artifact(entry: &OriginEntry) -> ServerListenArtifact {
    ServerListenArtifact {
        origin_id: entry.id.clone(),
        name: entry.name.clone(),
        port: listen_port(&entry.name),
        env: listen_env_from_name(&entry.name),
    }
}

fn listen_port(name: &str) -> Option<u16> {
    name.strip_prefix("port ")?.parse::<u16>().ok()
}

fn listen_env_from_name(name: &str) -> Option<ServerListenEnvArtifact> {
    let rest = name.strip_prefix("port env ")?;
    let (variable, default_port) = if let Some((variable, default)) = rest.split_once(" default ") {
        (variable, default.parse::<u16>().ok())
    } else {
        (rest, None)
    };
    Some(ServerListenEnvArtifact {
        variable: variable.to_string(),
        default_port,
    })
}

pub fn content_hash(source: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

pub fn runtime_features(
    origin_map: &OriginMap,
    has_server: bool,
    server_routes: usize,
) -> Vec<String> {
    let mut features = BTreeSet::new();
    if has_server {
        features.insert("http_server");
    }
    if server_routes > 0 {
        features.insert("router");
    }
    for entry in &origin_map.entries {
        match (entry.kind.as_str(), entry.name.as_str()) {
            ("route", route) if route_has_default_rate_limit(route) => {
                features.insert("rate_limit");
            }
            ("domain", "db") => {
                features.insert("in_memory_db");
            }
            ("call", "@db.connect") => {
                features.insert("db_adapter");
            }
            ("call", "@payment.connect") => {
                features.insert("payment_adapter");
            }
            ("call", "@shipping.connect") => {
                features.insert("shipping_adapter");
            }
            ("domain", "html") => {
                features.insert("html_renderer");
            }
            ("domain", "out") => {
                features.insert("console_io");
            }
            ("domain", "serve") => {
                features.insert("static_file_server");
            }
            ("domain", "csrf") => {
                features.insert("csrf_protection");
            }
            ("domain", "session") => {
                features.insert("session_cookies");
            }
            ("domain", "Auth") => {
                features.insert("auth_roles");
            }
            ("domain", "rateLimit") => {
                features.insert("rate_limit");
            }
            _ => {}
        }
    }
    features.into_iter().map(str::to_string).collect()
}

fn route_has_default_rate_limit(route: &str) -> bool {
    matches!(
        route.split_once(' '),
        Some(("POST", "/members/login" | "/checkout" | "/webhooks/stripe"))
    )
}
