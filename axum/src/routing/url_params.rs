use crate::util::{ByteStr, PercentDecodedByteStr};
use http::Extensions;
use matchit::Params;
use std::{borrow::Cow, collections::HashMap, sync::Arc};

pub(crate) enum UrlParams {
    Params(Vec<(ByteStr, PercentDecodedByteStr)>),
    InvalidUtf8InPathParam { key: ByteStr },
}

pub(super) fn insert_url_params(
    extensions: &mut Extensions,
    params: Params,
    matched_path: &str,
    param_mapping: &NormalizePathParams,
) {
    let current_params = extensions.get_mut();

    if let Some(UrlParams::InvalidUtf8InPathParam { .. }) = current_params {
        // nothing to do here since an error was stored earlier
        return;
    }

    let params = params
        .iter()
        .map(|(key, value)| {
            (
                param_mapping.get_original_param_for_path(matched_path, key),
                value,
            )
        })
        .filter(|(key, _)| !key.starts_with(super::NEST_TAIL_PARAM))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .map(|(k, v)| {
            if let Some(decoded) = PercentDecodedByteStr::new(v) {
                Ok((ByteStr::new(k), decoded))
            } else {
                Err(ByteStr::new(k))
            }
        })
        .collect::<Result<Vec<_>, _>>();

    match (current_params, params) {
        (Some(UrlParams::InvalidUtf8InPathParam { .. }), _) => {
            unreachable!("we check for this state earlier in this method")
        }
        (_, Err(invalid_key)) => {
            extensions.insert(UrlParams::InvalidUtf8InPathParam { key: invalid_key });
        }
        (Some(UrlParams::Params(current)), Ok(params)) => {
            current.extend(params);
        }
        (None, Ok(params)) => {
            extensions.insert(UrlParams::Params(params));
        }
    }
}

/// Workaround for https://github.com/tokio-rs/axum/issues/678
///
/// matchit considers these routes to overlap
/// - /:a
/// - /:b/:c
///
/// The solution is to change the routes to have a common prefix:
/// - /:a
/// - /:a/:c
///
/// `NormalizePathParams` does that by normalizing each param so `/:a/:b/:c` becomes
/// `/:axum_internal_param_0/:axum_internal_param_1/:axum_internal_param_2`.
///
/// It also supports mapping back to the original param names required by `Path` and `MatchedPath`.
///
/// Ideally matchit would handle this automatically (https://github.com/ibraheemdev/matchit/issues/13)
#[derive(Default, Clone, Debug)]
pub(super) struct NormalizePathParams {
    map: Arc<HashMap<String, OriginalPathAndNormalizedParams>>,
}

#[derive(Clone, Debug)]
struct OriginalPathAndNormalizedParams {
    original_path: String,
    normalized_params: HashMap<String, String>,
}

const PARAM_PREFIX: &str = "axum_internal_param_";

impl NormalizePathParams {
    pub(super) fn normalize_route_params(&mut self, path: &str) -> String {
        let mut normalized_params = HashMap::<String, String>::new();

        let normalized_path = path
            .split('/')
            .enumerate()
            .map(|(idx, segment)| -> Cow<_> {
                if let Some(param) = segment.strip_prefix(':') {
                    let normalized_param_name = format!("{}{}", PARAM_PREFIX, idx);

                    normalized_params.insert(normalized_param_name.clone(), param.into());

                    format!(":{}", normalized_param_name).into()
                } else if let Some(param) = segment.strip_prefix('*') {
                    let normalized_param_name = format!("{}{}", PARAM_PREFIX, idx);

                    normalized_params.insert(normalized_param_name.clone(), param.into());

                    format!("*{}", normalized_param_name).into()
                } else {
                    segment.into()
                }
            })
            .collect::<Vec<_>>()
            .join("/");

        self.update_map(|map| {
            map.insert(
                normalized_path.clone(),
                OriginalPathAndNormalizedParams {
                    original_path: path.to_owned(),
                    normalized_params,
                },
            );
        });

        normalized_path
    }

    pub(super) fn get_original_path(&self, matched_path: &str) -> &str {
        &self.map.get(matched_path).unwrap().original_path
    }

    fn get_original_param_for_path(&self, matched_path: &str, normalized_param: &str) -> &str {
        self.map
            .get(matched_path)
            .unwrap()
            .normalized_params
            .get(normalized_param)
            .unwrap()
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.update_map(|map| map.extend(other.map.as_ref().clone()));
    }

    fn update_map<F>(&mut self, f: F)
    where
        F: FnOnce(&mut HashMap<String, OriginalPathAndNormalizedParams>),
    {
        let mut map = self.map.as_ref().clone();
        f(&mut map);
        self.map = Arc::new(map);
    }
}
