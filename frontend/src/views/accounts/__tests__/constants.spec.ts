import { describe, expect, it } from 'vitest'
import { computeColumnLayout, resolveColumns } from '@/components/base/BaseTable/columns'
import { accountColumns } from '../constants'

describe('accountColumns', () => {
  it('在 1440px 视口对应的 1100px 容器内不产生横向滚动', () => {
    const columns = resolveColumns(accountColumns)
    const minWidthTotal = columns.reduce((total, column) => total + column.minWidth, 0)
    const layout = computeColumnLayout(columns, 1100)

    expect(minWidthTotal).toBeLessThanOrEqual(1100)
    expect(layout.tableWidth).toBe(1100)
  })

  it('将最后使用、创建时间和更新时间合并为时间分组列', () => {
    expect(accountColumns.find(column => column.key === 'lastUsedAt')).toMatchObject({
      label: '最后使用',
      kind: 'datetime',
      minWidth: 132,
    })
  })
})
