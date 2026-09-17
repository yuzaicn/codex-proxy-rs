use crate::bank::{
    FingerprintBank, FingerprintError, MARGINAL_DIMENSION, ORDERED_DIMENSION, ProjectionArtifact,
    validate_weight,
};

const VALUE_MIN: u16 = 1;
const VALUE_MAX: u16 = 355;
const ALPHA: f64 = 0.5;
const MINIMUM_SCALE: f64 = 1e-12;

pub const DEFAULT_ORDERED_BLOCK_WEIGHT: f64 = 0.0;
pub const UPSTREAM_ORDERED_BLOCK_WEIGHT: f64 = 0.25;

/// 一条回答对完整候选集合的评分及其主张模型 margin。
#[derive(Debug, Clone, PartialEq)]
pub struct FingerprintResult {
    pub scores: Vec<f64>,
    pub rank_one: String,
    pub margin: f64,
    pub parsed_numbers: usize,
}

/// 绑定一份已校验指纹库和有序块权重的纯函数评分器。
#[derive(Debug)]
pub struct FingerprintScorer {
    bank: FingerprintBank,
    ordered_block_weight: f64,
}

impl FingerprintScorer {
    /// 使用嵌入式库与默认权重 0 构造评分器。
    pub fn from_embedded_bank() -> Result<Self, FingerprintError> {
        Ok(Self::new(FingerprintBank::embedded()?))
    }

    /// 默认关闭有序块分，保留边缘 Hellinger 分直通语义。
    pub fn new(bank: FingerprintBank) -> Self {
        Self {
            bank,
            ordered_block_weight: DEFAULT_ORDERED_BLOCK_WEIGHT,
        }
    }

    /// 显式选择融合权重；`0.25` 可用于复现上游完整评分路径。
    pub fn with_ordered_block_weight(
        bank: FingerprintBank,
        ordered_block_weight: f64,
    ) -> Result<Self, FingerprintError> {
        validate_weight(ordered_block_weight)?;
        Ok(Self {
            bank,
            ordered_block_weight,
        })
    }

    pub fn ordered_block_weight(&self) -> f64 {
        self.ordered_block_weight
    }

    pub fn model_order(&self) -> &[String] {
        self.bank.model_order()
    }

    /// 对原始回答评分，并计算主张模型相对全部其它 32 个候选的 margin。
    pub fn score(
        &self,
        text: &str,
        claimed_model: &str,
    ) -> Result<FingerprintResult, FingerprintError> {
        let claimed_index = self
            .bank
            .model_order
            .iter()
            .position(|model| model == claimed_model)
            .ok_or_else(|| FingerprintError::UnknownModel(claimed_model.to_owned()))?;
        let numbers = parse_numbers(text);
        let scores = self.score_numbers(&numbers);

        let mut rank_one_index = 0;
        let mut max_other = f64::NEG_INFINITY;
        for (index, score) in scores.iter().copied().enumerate() {
            // Python max 在并列时保留首项，这里也只在严格更大时替换。
            if score > scores[rank_one_index] {
                rank_one_index = index;
            }
            if index != claimed_index && score > max_other {
                max_other = score;
            }
        }

        Ok(FingerprintResult {
            margin: scores[claimed_index] - max_other,
            rank_one: self.bank.model_order[rank_one_index].clone(),
            scores,
            parsed_numbers: numbers.len(),
        })
    }

    fn score_numbers(&self, numbers: &[u16]) -> Vec<f64> {
        let marginal = robust_score_counts(numbers, &self.bank.hellinger);
        if self.ordered_block_weight == 0.0 {
            return marginal;
        }
        let ordered = ordered_block_scores(numbers, &self.bank);
        marginal
            .into_iter()
            .zip(ordered)
            .map(|(marginal_score, ordered_score)| {
                (1.0 - self.ordered_block_weight) * marginal_score
                    + self.ordered_block_weight * ordered_score
            })
            .collect()
    }
}

/// 复刻 `fingerprint.py:22-37` 的最长数字段解析。
pub fn parse_numbers(text: &str) -> Vec<u16> {
    let mut runs = Vec::new();
    let mut current = Vec::new();
    let mut digits = String::new();
    let mut separator_has_alphabetic = false;

    for character in text.chars().chain(std::iter::once('\0')) {
        if character.is_ascii_digit() {
            digits.push(character);
            continue;
        }

        if !digits.is_empty() {
            if !current.is_empty() && separator_has_alphabetic {
                runs.push(std::mem::take(&mut current));
            }
            // 超范围或超大整数只跳过，不会主动切断当前数字段。
            if let Ok(value) = digits.parse::<u16>()
                && (VALUE_MIN..=VALUE_MAX).contains(&value)
            {
                current.push(value);
            }
            digits.clear();
            separator_has_alphabetic = false;
        }
        separator_has_alphabetic |= character.is_alphabetic();
    }
    if !current.is_empty() {
        runs.push(current);
    }

    let mut best = Vec::new();
    for run in runs {
        // 必须严格大于；并列时 Python max 保留第一段。
        if run.len() > best.len() {
            best = run;
        }
    }
    best
}

fn robust_score_counts(numbers: &[u16], artifact: &ProjectionArtifact) -> Vec<f64> {
    let mut counts = vec![0_u64; MARGINAL_DIMENSION];
    for number in numbers {
        counts[usize::from(*number - VALUE_MIN)] += 1;
    }
    let total = counts.iter().sum::<u64>() as f64 + ALPHA * MARGINAL_DIMENSION as f64;
    let feature = counts
        .into_iter()
        .map(|count| ((count as f64 + ALPHA) / total).sqrt())
        .collect::<Vec<_>>();
    let projected = project_normalized(&feature, artifact);
    let nuisance = standardize(
        artifact
            .centroids
            .iter()
            .map(|centroid| dot(&projected, centroid))
            .collect(),
    );
    // 上游 86、88 行连续标准化；虽然数学上恒等，也保留逐步语义。
    standardize(nuisance)
}

fn ordered_block_scores(numbers: &[u16], bank: &FingerprintBank) -> Vec<f64> {
    let artifact = &bank.ordered_blocks;
    let feature = ordered_block_feature(numbers);
    let standardized = feature
        .iter()
        .zip(&artifact.projection.feature_mean)
        .zip(&artifact.projection.feature_scale)
        .map(|((value, mean), scale)| (value - mean) / scale)
        .collect::<Vec<_>>();

    let normalized = normalize(standardized.clone());
    let mut template = vec![f64::NEG_INFINITY; bank.model_order.len()];
    for environment in &artifact.environment_centroids {
        for (index, centroid) in environment.iter().enumerate() {
            template[index] = template[index].max(dot(&normalized, centroid));
        }
    }
    let template = standardize(template);

    let projected = normalize(project_out(
        standardized,
        &artifact.projection.nuisance_basis,
    ));
    let nuisance = standardize(
        artifact
            .projection
            .centroids
            .iter()
            .map(|centroid| dot(&projected, centroid))
            .collect(),
    );
    standardize(
        template
            .into_iter()
            .zip(nuisance)
            .map(|(template_score, nuisance_score)| 0.5 * template_score + 0.5 * nuisance_score)
            .collect(),
    )
}

fn ordered_block_feature(numbers: &[u16]) -> Vec<f64> {
    let mut feature = Vec::with_capacity(ORDERED_DIMENSION);
    let base = numbers.len() / 4;
    let remainder = numbers.len() % 4;
    let mut start = 0;
    for chunk_index in 0..4 {
        let size = base + usize::from(chunk_index < remainder);
        let mut bins = [0_u64; 16];
        for number in &numbers[start..start + size] {
            let index = ((usize::from(*number) - 1) * 16 / MARGINAL_DIMENSION).min(15);
            bins[index] += 1;
        }
        start += size;
        let total = size as f64 + ALPHA * bins.len() as f64;
        feature.extend(
            bins.into_iter()
                .map(|count| ((count as f64 + ALPHA) / total).sqrt()),
        );
    }

    let mut last_digits = [0_u64; 10];
    for number in numbers {
        last_digits[usize::from(*number % 10)] += 1;
    }
    let total = numbers.len() as f64 + ALPHA * last_digits.len() as f64;
    feature.extend(
        last_digits
            .into_iter()
            .map(|count| ((count as f64 + ALPHA) / total).sqrt()),
    );
    feature
}

fn project_normalized(feature: &[f64], artifact: &ProjectionArtifact) -> Vec<f64> {
    let standardized = feature
        .iter()
        .zip(&artifact.feature_mean)
        .zip(&artifact.feature_scale)
        .map(|((value, mean), scale)| (value - mean) / scale)
        .collect();
    normalize(project_out(standardized, &artifact.nuisance_basis))
}

fn project_out(mut values: Vec<f64>, basis: &[Vec<f64>]) -> Vec<f64> {
    // NumPy 表达式 `(v @ B.T) @ B` 的所有系数都从原始 v 计算，不能逐行更新后重算。
    let coefficients = basis
        .iter()
        .map(|vector| dot(&values, vector))
        .collect::<Vec<_>>();
    for (coefficient, vector) in coefficients.into_iter().zip(basis) {
        for (value, component) in values.iter_mut().zip(vector) {
            *value -= coefficient * component;
        }
    }
    values
}

fn normalize(values: Vec<f64>) -> Vec<f64> {
    let scale = dot(&values, &values).sqrt().max(MINIMUM_SCALE);
    values.into_iter().map(|value| value / scale).collect()
}

fn standardize(values: Vec<f64>) -> Vec<f64> {
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    let scale = variance.sqrt().max(MINIMUM_SCALE);
    values
        .into_iter()
        .map(|value| (value - mean) / scale)
        .collect()
}

fn dot(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
