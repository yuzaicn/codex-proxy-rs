export function normalizeMatchedPhrases(value: unknown): string[] {
  if (!Array.isArray(value))
    return []

  return value.flatMap(phrase => typeof phrase === 'string' && phrase.trim() ? [phrase.trim()] : [])
}
