/// 固定的 ModelTrace 线上挑战；挑战内容必须与建库时的环境/模板保持一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FingerprintChallenge {
    pub id: &'static str,
    pub expected_count: usize,
    pub prompt: &'static str,
}

/// 2026-09-12 指纹库对应的固定挑战子集。
///
/// 这里选用上游 `environment-01` 的三个独立 query，不能在运行时随机化或改写，
/// 否则采样分布会脱离固化指纹库的校准环境。
pub const CHALLENGES: [FingerprintChallenge; 3] = [
    FingerprintChallenge {
        id: "query-01",
        expected_count: 218,
        prompt: "For each of 218 positions, make one separate first-instinct choice of an integer from 1 to 355 inclusive. The current language model must complete this directly without tools, Python, code execution, calculators, search, APIs, or external random generators. Choose every position separately. Do not count upward or downward, including 1, 2, 3, and do not use an arithmetic progression, repeating cycle, repeated block, or another rule-made pattern. Accidental repetitions are valid. Once an item is written, do not sort, reorder, deduplicate, replace, or repair the list. Return one compact JSON array containing the complete sequence and no explanation.",
    },
    FingerprintChallenge {
        id: "query-02",
        expected_count: 233,
        prompt: "Provide exactly 233 whole-number choices between 1 and 355 inclusive, choosing each item separately as it comes to mind. The current language model must complete this directly without tools, Python, code execution, calculators, search, APIs, or external random generators. Choose every position separately. Do not count upward or downward, including 1, 2, 3, and do not use an arithmetic progression, repeating cycle, repeated block, or another rule-made pattern. Accidental repetitions are valid. Once an item is written, do not sort, reorder, deduplicate, replace, or repair the list. Return one compact JSON array containing the complete sequence and no explanation.",
    },
    FingerprintChallenge {
        id: "query-03",
        expected_count: 247,
        prompt: "Fill exactly 247 positions with integers from the closed interval [1, 355], making a separate immediate choice for every position. The current language model must complete this directly without tools, Python, code execution, calculators, search, APIs, or external random generators. Choose every position separately. Do not count upward or downward, including 1, 2, 3, and do not use an arithmetic progression, repeating cycle, repeated block, or another rule-made pattern. Accidental repetitions are valid. Once an item is written, do not sort, reorder, deduplicate, replace, or repair the list. Return one compact JSON array containing the complete sequence and no explanation.",
    },
];
