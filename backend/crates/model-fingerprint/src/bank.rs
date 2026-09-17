use std::{collections::BTreeSet, error::Error, fmt};

use serde::Deserialize;

const EMBEDDED_BANK: &[u8] = include_bytes!("../assets/modeltrace-bank-20260912.json");
pub(crate) const MARGINAL_DIMENSION: usize = 355;
pub(crate) const ORDERED_DIMENSION: usize = 74;

/// 已校验、可用于评分的 ModelTrace 指纹库。
#[derive(Debug)]
pub struct FingerprintBank {
    built_at: String,
    pub(crate) model_order: Vec<String>,
    pub(crate) hellinger: ProjectionArtifact,
    pub(crate) ordered_blocks: OrderedBlockArtifact,
}

#[derive(Debug)]
pub(crate) struct ProjectionArtifact {
    pub(crate) feature_mean: Vec<f64>,
    pub(crate) feature_scale: Vec<f64>,
    pub(crate) nuisance_basis: Vec<Vec<f64>>,
    pub(crate) centroids: Vec<Vec<f64>>,
}

#[derive(Debug)]
pub(crate) struct OrderedBlockArtifact {
    pub(crate) upstream_weight: f64,
    pub(crate) projection: ProjectionArtifact,
    pub(crate) environment_centroids: Vec<Vec<Vec<f64>>>,
}

/// 指纹库或评分参数不满足固定合同。
#[derive(Debug)]
pub enum FingerprintError {
    InvalidBankJson(serde_json::Error),
    InvalidBank(String),
    InvalidOrderedBlockWeight(f64),
    UnknownModel(String),
}

impl fmt::Display for FingerprintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBankJson(error) => {
                write!(formatter, "invalid fingerprint bank JSON: {error}")
            }
            Self::InvalidBank(reason) => write!(formatter, "invalid fingerprint bank: {reason}"),
            Self::InvalidOrderedBlockWeight(weight) => {
                write!(
                    formatter,
                    "ordered block weight must be finite and within 0..=1, got {weight}"
                )
            }
            Self::UnknownModel(model) => write!(
                formatter,
                "model is not present in fingerprint bank: {model}"
            ),
        }
    }
}

impl Error for FingerprintError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidBankJson(error) => Some(error),
            Self::InvalidBank(_) | Self::InvalidOrderedBlockWeight(_) | Self::UnknownModel(_) => {
                None
            }
        }
    }
}

impl FingerprintBank {
    /// 解析编译进二进制的 2026-09-12 指纹库。
    pub fn embedded() -> Result<Self, FingerprintError> {
        Self::from_json(EMBEDDED_BANK)
    }

    /// 解析并完整校验一个指纹库；任何损坏都作为错误返回，不产生默认判定。
    pub fn from_json(bytes: &[u8]) -> Result<Self, FingerprintError> {
        let raw: RawBank =
            serde_json::from_slice(bytes).map_err(FingerprintError::InvalidBankJson)?;
        raw.validate()
    }

    pub fn built_at(&self) -> &str {
        &self.built_at
    }

    pub fn model_order(&self) -> &[String] {
        &self.model_order
    }

    pub fn upstream_ordered_block_weight(&self) -> f64 {
        self.ordered_blocks.upstream_weight
    }
}

#[derive(Deserialize)]
struct RawBank {
    built_at: String,
    models: Vec<RawModel>,
    robust: RawRobust,
}

#[derive(Deserialize)]
struct RawModel {
    id: String,
}

#[derive(Deserialize)]
struct RawRobust {
    model_order: Vec<String>,
    hellinger: RawProjectionArtifact,
    ordered_blocks: RawOrderedBlockArtifact,
}

#[derive(Deserialize)]
struct RawProjectionArtifact {
    feature_mean: Vec<f64>,
    feature_scale: Vec<f64>,
    nuisance_basis: Vec<Vec<f64>>,
    centroids: Vec<Vec<f64>>,
}

#[derive(Deserialize)]
struct RawOrderedBlockArtifact {
    weight: f64,
    feature_mean: Vec<f64>,
    feature_scale: Vec<f64>,
    nuisance_basis: Vec<Vec<f64>>,
    centroids: Vec<Vec<f64>>,
    environment_centroids: Vec<Vec<Vec<f64>>>,
}

impl RawBank {
    fn validate(self) -> Result<FingerprintBank, FingerprintError> {
        if self.built_at.trim().is_empty() {
            return Err(invalid("built_at must not be empty"));
        }
        if self.robust.model_order.len() < 2 {
            return Err(invalid("model_order must contain at least two models"));
        }

        let model_ids = self
            .models
            .into_iter()
            .map(|model| model.id)
            .collect::<Vec<_>>();
        if model_ids != self.robust.model_order {
            return Err(invalid(
                "robust.model_order must exactly match models[].id order",
            ));
        }
        let unique = model_ids.iter().collect::<BTreeSet<_>>();
        if unique.len() != model_ids.len() {
            return Err(invalid("model ids must be unique"));
        }

        let model_count = model_ids.len();
        let hellinger = validate_projection(
            "robust.hellinger",
            self.robust.hellinger,
            MARGINAL_DIMENSION,
            model_count,
        )?;
        let ordered = self.robust.ordered_blocks;
        validate_weight(ordered.weight)?;
        let projection = validate_projection(
            "robust.ordered_blocks",
            RawProjectionArtifact {
                feature_mean: ordered.feature_mean,
                feature_scale: ordered.feature_scale,
                nuisance_basis: ordered.nuisance_basis,
                centroids: ordered.centroids,
            },
            ORDERED_DIMENSION,
            model_count,
        )?;
        if ordered.environment_centroids.is_empty() {
            return Err(invalid(
                "robust.ordered_blocks.environment_centroids must not be empty",
            ));
        }
        validate_tensor(
            "robust.ordered_blocks.environment_centroids",
            &ordered.environment_centroids,
            model_count,
            ORDERED_DIMENSION,
        )?;

        Ok(FingerprintBank {
            built_at: self.built_at,
            model_order: model_ids,
            hellinger,
            ordered_blocks: OrderedBlockArtifact {
                upstream_weight: ordered.weight,
                projection,
                environment_centroids: ordered.environment_centroids,
            },
        })
    }
}

fn validate_projection(
    label: &str,
    raw: RawProjectionArtifact,
    feature_dimension: usize,
    model_count: usize,
) -> Result<ProjectionArtifact, FingerprintError> {
    validate_vector(
        &format!("{label}.feature_mean"),
        &raw.feature_mean,
        feature_dimension,
    )?;
    validate_vector(
        &format!("{label}.feature_scale"),
        &raw.feature_scale,
        feature_dimension,
    )?;
    if raw.feature_scale.iter().any(|scale| *scale <= 0.0) {
        return Err(invalid(format!("{label}.feature_scale must be positive")));
    }
    validate_matrix(
        &format!("{label}.nuisance_basis"),
        &raw.nuisance_basis,
        feature_dimension,
    )?;
    if raw.centroids.len() != model_count {
        return Err(invalid(format!(
            "{label}.centroids must have {model_count} rows, got {}",
            raw.centroids.len()
        )));
    }
    validate_matrix(
        &format!("{label}.centroids"),
        &raw.centroids,
        feature_dimension,
    )?;
    Ok(ProjectionArtifact {
        feature_mean: raw.feature_mean,
        feature_scale: raw.feature_scale,
        nuisance_basis: raw.nuisance_basis,
        centroids: raw.centroids,
    })
}

fn validate_tensor(
    label: &str,
    values: &[Vec<Vec<f64>>],
    expected_rows: usize,
    expected_columns: usize,
) -> Result<(), FingerprintError> {
    for (index, matrix) in values.iter().enumerate() {
        if matrix.len() != expected_rows {
            return Err(invalid(format!(
                "{label}[{index}] must have {expected_rows} rows, got {}",
                matrix.len()
            )));
        }
        validate_matrix(&format!("{label}[{index}]"), matrix, expected_columns)?;
    }
    Ok(())
}

fn validate_matrix(
    label: &str,
    values: &[Vec<f64>],
    expected_columns: usize,
) -> Result<(), FingerprintError> {
    for (index, row) in values.iter().enumerate() {
        validate_vector(&format!("{label}[{index}]"), row, expected_columns)?;
    }
    Ok(())
}

fn validate_vector(label: &str, values: &[f64], expected: usize) -> Result<(), FingerprintError> {
    if values.len() != expected {
        return Err(invalid(format!(
            "{label} must have dimension {expected}, got {}",
            values.len()
        )));
    }
    if values.iter().any(|value| !value.is_finite()) {
        return Err(invalid(format!("{label} contains a non-finite value")));
    }
    Ok(())
}

pub(crate) fn validate_weight(weight: f64) -> Result<(), FingerprintError> {
    if weight.is_finite() && (0.0..=1.0).contains(&weight) {
        Ok(())
    } else {
        Err(FingerprintError::InvalidOrderedBlockWeight(weight))
    }
}

fn invalid(reason: impl Into<String>) -> FingerprintError {
    FingerprintError::InvalidBank(reason.into())
}
