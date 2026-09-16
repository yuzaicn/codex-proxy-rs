//! ModelTrace 数字指纹评分内核。
//!
//! 本 crate 固化上游 ModelTrace `bff6cb1ff6f91e70a4e61d3f106f8c4925918eea`
//! 的 33 模型指纹库，并复刻 `fingerprint.py:22-131` 的纯数值评分。默认
//! `ordered_block_weight = 0`，偏离上游的 `0.25`；依据 GUCH-352 §4.2 的消融，
//! 第一版只让边缘分参与判定，但完整保留有序块实现供后续基线复验。上游 468 条建库
//! 数据按环境聚合为 156 组三份回答后，两种权重的自评 rank-1 都是 1.0000，margin
//! 均值分别为 1.6232 和 1.6509；这只是拟合数据上界，不是诚实留出结论。
//!
//! 随库 `provenance.json` 明示 `sameContextCalibrated: false` 与
//! `multilingualCalibrated: false`，因此调用方仍需使用自己的账号基线校准判定阈值。

mod bank;
mod scoring;

pub use bank::{FingerprintBank, FingerprintError};
pub use scoring::{
    DEFAULT_ORDERED_BLOCK_WEIGHT, FingerprintResult, FingerprintScorer,
    UPSTREAM_ORDERED_BLOCK_WEIGHT, parse_numbers,
};
