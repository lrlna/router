use crate::*;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

/// A graphql primary response.
/// Used for federated and subgraph queries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, TypedBuilder)]
#[serde(rename_all = "camelCase")]
#[builder(field_defaults(setter(into)))]
pub struct GraphQLResponse {
    /// The label that was passed to the defer or stream directive for this patch.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[builder(default)]
    pub label: Option<String>,

    /// The response data.
    #[serde(skip_serializing_if = "skip_data_if", default)]
    #[builder(default = Value::Object(Default::default()))]
    pub data: Value,

    /// The path that the data should be merged at.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[builder(default)]
    pub path: Option<Path>,

    /// The optional indicator that there may be more data in the form of a patch response.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[builder(default)]
    pub has_next: Option<bool>,

    /// The optional graphql errors encountered.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[builder(default)]
    pub errors: Vec<GraphQLError>,

    /// The optional graphql extensions.
    #[serde(skip_serializing_if = "Object::is_empty", default)]
    #[builder(default)]
    pub extensions: Object,
}

fn skip_data_if(value: &Value) -> bool {
    match value {
        Value::Object(o) => o.is_empty(),
        Value::Null => true,
        _ => false,
    }
}

impl GraphQLResponse {
    pub fn is_primary(&self) -> bool {
        self.has_next.is_none()
    }

    pub fn select(&self, path: &Path, selections: &[Selection]) -> Result<Value, FetchError> {
        let values =
            self.data
                .get_at_path(path)
                .map_err(|err| FetchError::ExecutionPathNotFound {
                    reason: err.to_string(),
                })?;

        Ok(Value::Array(
            values
                .into_iter()
                .flat_map(|value| match (value, selections) {
                    (Value::Object(content), requires) => {
                        Some(select_object(content, requires).transpose().expect("todo"))
                    }
                    (_, _) => Some(Err(FetchError::ExecutionInvalidContent {
                        reason: "not an object".to_string(),
                    })),
                })
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    pub fn insert_data(&mut self, path: &Path, value: Value) -> Result<(), FetchError> {
        let nodes =
            self.data
                .get_at_path_mut(path)
                .map_err(|err| FetchError::ExecutionPathNotFound {
                    reason: err.to_string(),
                })?;

        for node in nodes {
            node.deep_merge(value.clone());
        }

        Ok(())
    }

    /// TODO
    pub fn merge(&mut self, mut other: Self) {
        if let Some(path) = other.path.as_ref() {
            self.insert_data(path, other.data).expect("todo");
        } else {
            self.data.deep_merge(other.data);
        }

        self.errors.append(&mut other.errors);
    }
}

fn select_object(content: &Object, selections: &[Selection]) -> Result<Option<Value>, FetchError> {
    let mut output = Object::new();
    for selection in selections {
        match selection {
            Selection::Field(field) => {
                if let Some(value) = select_field(content, field)? {
                    if let Some(existing) = output.get_mut(&field.name) {
                        existing.deep_merge(value);
                    } else {
                        output.insert(field.name.to_owned(), value);
                    }
                }
            }
            Selection::InlineFragment(fragment) => {
                if let Some(Value::Object(value)) = select_inline_fragment(content, fragment)? {
                    output.append(&mut value.to_owned())
                }
            }
        };
    }
    if output.is_empty() {
        return Ok(None);
    }
    Ok(Some(Value::Object(output)))
}

fn select_field(content: &Object, field: &Field) -> Result<Option<Value>, FetchError> {
    match (content.get(&field.name), &field.selections) {
        (Some(Value::Object(child)), Some(selections)) => select_object(child, selections),
        (Some(value), None) => Ok(Some(value.to_owned())),
        (None, _) => Err(FetchError::ExecutionFieldNotFound {
            field: field.name.to_owned(),
        }),
        _ => Ok(None),
    }
}

fn select_inline_fragment(
    content: &Object,
    fragment: &InlineFragment,
) -> Result<Option<Value>, FetchError> {
    match (&fragment.type_condition, &content.get("__typename")) {
        (Some(condition), Some(Value::String(typename))) => {
            if condition == typename {
                select_object(content, &fragment.selections)
            } else {
                Ok(None)
            }
        }
        (None, _) => select_object(content, &fragment.selections),
        (_, None) => Err(FetchError::ExecutionFieldNotFound {
            field: "__typename".to_string(),
        }),
        (_, _) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    macro_rules! select {
        ($content:expr $(,)?) => {{
            let response = GraphQLResponse::builder()
                .data($content)
                .build();
            let stub = json!([
                {
                    "kind": "InlineFragment",
                    "typeCondition": "OtherStuffToIgnore",
                    "selections": [],
                },
                {
                    "kind": "InlineFragment",
                    "typeCondition": "User",
                    "selections": [
                        {
                            "kind": "Field",
                            "name": "__typename",
                        },
                        {
                            "kind": "Field",
                            "name": "id",
                        },
                        {
                            "kind": "Field",
                            "name": "job",
                            "selections": [
                                {
                                    "kind": "Field",
                                    "name": "name",
                                }
                            ],
                        }
                      ]
                },
            ]);
            let selection: Vec<Selection> = serde_json::from_value(stub).unwrap();
            response.select(&Path::empty(), &selection)
        }};
    }

    #[test]
    fn test_selection() {
        assert_eq!(
            select!(
                json!({"__typename": "User", "id":2, "name":"Bob", "job":{"name":"astronaut"}}),
            )
            .unwrap(),
            json!([{
                "__typename": "User",
                "id": 2,
                "job": {
                    "name": "astronaut"
                }
            }]),
        );
    }

    #[test]
    fn test_selection_missing_field() {
        assert!(matches!(
            select!(json!({"__typename": "User", "name":"Bob", "job":{"name":"astronaut"}}))
                .unwrap_err(),
            FetchError::ExecutionFieldNotFound { field } if field == "id"
        ));
    }

    #[test]
    fn test_insert_data() {
        let mut response = GraphQLResponse::builder()
            .data(json!({
                "name": "SpongeBob",
                "job": {},
            }))
            .build();
        response
            .insert_data(
                &Path::from("job"),
                json!({
                    "name": "cook",
                }),
            )
            .unwrap();
        assert_eq!(
            response.data,
            json!({
                "name": "SpongeBob",
                "job": {
                    "name": "cook",
                },
            }),
        );
    }
}
