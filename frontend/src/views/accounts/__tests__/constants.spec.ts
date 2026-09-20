import { describe, expect, it } from 'vitest'
import { columnSortKey, computeColumnLayout, resolveColumns } from '@/components/base/BaseTable/columns'
import { accountColumns, accountTimelineLabels } from '../constants'

describe('accountColumns', () => {
  it('在 1440px 视口对应的 1100px 容器内不产生横向滚动', () => {
    const columns = resolveColumns(accountColumns)
    const minWidthTotal = columns.reduce((total, column) => total + column.minWidth, 0)
    const layout = computeColumnLayout(columns, 1100)

    expect(minWidthTotal).toBeLessThanOrEqual(1100)
    expect(layout.tableWidth).toBe(1100)
  })

  it('用概括性组名展示三项时间，并保持按最后使用排序', () => {
    const timelineColumn = accountColumns.find(column => column.key === 'lastUsedAt')

    expect(timelineColumn).toMatchObject({
      label: '时间',
      kind: 'datetime',
      minWidth: 132,
      sortable: true,
    })
    expect(accountTimelineLabels).toHaveLength(3)
    expect(accountTimelineLabels).toContain('最后使用')
    expect(accountTimelineLabels.filter(label => label.endsWith('时间'))).toHaveLength(2)
    expect(columnSortKey(timelineColumn!)).toBe('lastUsedAt')
  })
})
