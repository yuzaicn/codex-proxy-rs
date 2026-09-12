export type TableRow = object

export type TableColumnKind
  = | 'text'
    | 'identity'
    | 'meta'
    | 'status'
    | 'numeric'
    | 'datetime'
    | 'mono'
    | 'index'
    | 'selection'
    | 'expander'
    | 'actions'
    | 'custom'

export type TableColumnSize = 'xs' | 'sm' | 'md' | 'lg' | 'xl' | '2xl' | '3xl' | '4xl'

export type TableColumnAlign = 'left' | 'right' | 'center'
type TableColumnSticky = 'left' | 'right'

export interface BaseTableColumn<Row extends TableRow = TableRow> {
  key: string
  label?: string
  kind?: TableColumnKind
  size?: TableColumnSize
  /** 容器变窄时该列允许压缩到的最小宽度（px）；缺省按 size 查压缩下限表，且不会超过列的基准宽度。 */
  minWidth?: number
  fixedWidth?: boolean
  align?: TableColumnAlign
  sortable?: boolean | string
  format?: (value: unknown, row: Row) => unknown
  emptyText?: string
}

export interface BaseTableSort {
  key: string
  direction: 'asc' | 'desc'
}

export interface BaseTableProps<Row extends TableRow> {
  columns: BaseTableColumn<Row>[]
  rows: Row[]
  rowKey?: string | ((row: Row, index: number) => string | number)
  selectedRowKeys?: Array<string | number>
  expandedRowKeys?: Array<string | number>
  density?: 'compact' | 'default'
  loading?: boolean
  emptyText?: string
  scrollbarAlwaysVisible?: boolean
  sort?: BaseTableSort
}

interface ColumnRecipe {
  size: TableColumnSize
  basisWidth?: number
  /** false 表示容器变宽时不参与拉伸（控制列与操作列固定观感，也让 sticky 偏移可预期）。 */
  stretch?: false
  align: TableColumnAlign
  truncate: boolean
  contentClass?: string
  paddingClass?: string
  sticky?: TableColumnSticky
}

const columnWidths: Record<TableColumnSize, number> = {
  'xs': 64,
  'sm': 88,
  'md': 112,
  'lg': 144,
  'xl': 184,
  '2xl': 240,
  '3xl': 288,
  '4xl': 352,
}

/**
 * 各档 size 的压缩下限：容器宽度不足以摆下全部基准宽度时，列先按比例压缩到这里，
 * 再触发横向滚动。下限保证常规内容（徽标、相对时间、可截断文本）仍可读。
 */
const columnMinWidths: Record<TableColumnSize, number> = {
  'xs': 56,
  'sm': 64,
  'md': 80,
  'lg': 108,
  'xl': 136,
  '2xl': 160,
  '3xl': 192,
  '4xl': 248,
}

const columnRecipes: Record<TableColumnKind, ColumnRecipe> = {
  text: {
    size: '2xl',
    align: 'left',
    truncate: true,
  },
  identity: {
    size: '2xl',
    align: 'left',
    truncate: true,
  },
  meta: {
    size: 'lg',
    align: 'left',
    truncate: true,
    contentClass: 'text-cp-text-secondary',
  },
  status: {
    size: 'md',
    align: 'center',
    truncate: false,
  },
  numeric: {
    size: 'md',
    align: 'right',
    truncate: false,
    contentClass: 'font-mono tabular-nums text-cp-text-secondary',
  },
  datetime: {
    size: 'xl',
    align: 'left',
    truncate: false,
    // 不再强制 nowrap：列压缩到不足一行时在“日期 时间”的空格处折行，避免溢出相邻列。
    contentClass: 'font-mono text-cp-sm leading-snug tabular-nums text-cp-text-secondary',
  },
  mono: {
    size: 'xl',
    align: 'left',
    truncate: true,
    contentClass: 'font-mono text-cp-sm font-emphasis',
  },
  index: {
    size: 'xs',
    align: 'center',
    truncate: false,
    contentClass: 'font-mono tabular-nums text-cp-text-secondary',
  },
  selection: {
    size: 'xs',
    basisWidth: 48,
    stretch: false,
    align: 'center',
    truncate: false,
    paddingClass: 'px-2',
    sticky: 'left',
  },
  expander: {
    size: 'xs',
    basisWidth: 40,
    stretch: false,
    align: 'center',
    truncate: false,
    paddingClass: 'px-2',
    sticky: 'left',
  },
  actions: {
    size: 'md',
    stretch: false,
    align: 'left',
    truncate: false,
    paddingClass: 'px-3',
    sticky: 'right',
  },
  custom: {
    size: 'lg',
    align: 'left',
    truncate: false,
  },
}

export interface ResolvedTableColumn<Row extends TableRow = TableRow>
  extends BaseTableColumn<Row> {
  kind: TableColumnKind
  basisWidth: number
  minWidth: number
  stretch: boolean
  align: TableColumnAlign
  truncate: boolean
  contentClass?: string
  paddingClass?: string
  sticky?: TableColumnSticky
}

export function defineTableColumns<Row extends TableRow>(columns: BaseTableColumn<Row>[]) {
  return columns
}

export function resolveColumns<Row extends TableRow>(
  columns: BaseTableColumn<Row>[],
): ResolvedTableColumn<Row>[] {
  return columns.map((column): ResolvedTableColumn<Row> => {
    const kind = column.kind ?? 'text'
    const recipe = columnRecipes[kind]
    const basisWidth = recipe.basisWidth ?? columnWidths[column.size ?? recipe.size]
    // fixedWidth 列与配方内置宽度的控制列（expander/selection）宽度恒定：不拉伸也不压缩。
    const pinned = column.fixedWidth === true || recipe.basisWidth !== undefined
    const minWidth = pinned
      ? basisWidth
      : Math.min(basisWidth, column.minWidth ?? columnMinWidths[column.size ?? recipe.size])

    return {
      ...column,
      kind,
      basisWidth,
      minWidth,
      stretch: pinned ? false : recipe.stretch !== false,
      align: column.align ?? recipe.align,
      truncate: recipe.truncate,
      contentClass: recipe.contentClass,
      paddingClass: recipe.paddingClass,
      sticky: recipe.sticky,
    }
  })
}

export interface TableColumnLayout {
  /** 每列最终宽度（px），与列数组一一对应，取整后合计恰为 tableWidth。 */
  widths: number[]
  tableWidth: number
  /** 容器宽度不足以摆下全部基准宽度、列处于压缩区间时为 true。 */
  compressed: boolean
}

/**
 * 由实测容器宽度分配各列像素宽度：
 * - 容器 ≥ Σ基准宽：可拉伸列按基准宽比例分享富余空间；
 * - Σ下限 ≤ 容器 < Σ基准宽：各列按自身可压缩余量（基准宽 − 下限）等比压缩，恰好填满容器；
 * - 容器 < Σ下限：各列取下限，表格宽于容器，交给横向滚动。
 * 尚未完成测量（containerWidth 为 null）时回退为基准宽度。
 */
export function computeColumnLayout<Row extends TableRow>(
  columns: ResolvedTableColumn<Row>[],
  containerWidth: number | null,
): TableColumnLayout {
  const basisTotal = columns.reduce((total, column) => total + column.basisWidth, 0)
  if (containerWidth === null || columns.length === 0)
    return { widths: columns.map(column => column.basisWidth), tableWidth: basisTotal, compressed: false }

  const minTotal = columns.reduce((total, column) => total + column.minWidth, 0)
  const tableWidth = Math.round(Math.max(containerWidth, minTotal))

  let targets: number[]
  if (tableWidth >= basisTotal) {
    const stretchable = columns.some(column => column.stretch)
    const stretchBasis = columns.reduce(
      (total, column) => total + (!stretchable || column.stretch ? column.basisWidth : 0),
      0,
    )
    const extra = tableWidth - basisTotal
    targets = columns.map(column =>
      (!stretchable || column.stretch) && stretchBasis > 0
        ? column.basisWidth + (extra * column.basisWidth) / stretchBasis
        : column.basisWidth,
    )
  }
  else {
    const slackTotal = columns.reduce((total, column) => total + (column.basisWidth - column.minWidth), 0)
    const deficit = basisTotal - tableWidth
    targets = columns.map(column =>
      column.basisWidth
      - (slackTotal > 0 ? (deficit * (column.basisWidth - column.minWidth)) / slackTotal : 0),
    )
  }

  // 逐列累计取整，保证列宽合计与表宽一致，sticky 偏移不会因舍入漂移。
  const widths: number[] = []
  let targetTotal = 0
  let assignedTotal = 0
  for (const target of targets) {
    targetTotal += target
    const width = Math.round(targetTotal - assignedTotal)
    widths.push(width)
    assignedTotal += width
  }

  return { widths, tableWidth: assignedTotal, compressed: tableWidth < basisTotal }
}

/** 按最终列宽计算 sticky 列偏移；与百分比时代不同，任何容器宽度下都与实际渲染宽度一致。 */
export function stickyColumnOffsets<Row extends TableRow>(
  columns: ResolvedTableColumn<Row>[],
  widths: number[],
): Array<number | undefined> {
  const offsets: Array<number | undefined> = Array.from({ length: columns.length })

  let leftOffset = 0
  columns.forEach((column, index) => {
    if (column.sticky !== 'left')
      return
    offsets[index] = leftOffset
    leftOffset += widths[index] ?? column.basisWidth
  })

  let rightOffset = 0
  for (let index = columns.length - 1; index >= 0; index -= 1) {
    const column = columns[index]
    if (column?.sticky !== 'right')
      continue
    offsets[index] = rightOffset
    rightOffset += widths[index] ?? column.basisWidth
  }

  return offsets
}

export function tableStyle(layout: TableColumnLayout, measured: boolean) {
  // 未测量时保持既有“max(100%, 基准宽合计)”行为，首帧观感与旧实现一致。
  return measured
    ? { width: `${layout.tableWidth}px` }
    : { width: `max(100%, ${layout.tableWidth}px)` }
}

export function columnStyle(layout: TableColumnLayout, index: number) {
  const width = layout.widths[index]
  return width === undefined ? undefined : { width: `${width}px` }
}

export function stickyStyle<Row extends TableRow>(
  column: ResolvedTableColumn<Row>,
  offset: number | undefined,
) {
  if (!column.sticky)
    return undefined
  return { [column.sticky]: `${offset ?? 0}px` }
}

export function alignClass<Row extends TableRow>(column: ResolvedTableColumn<Row>) {
  if (column.align === 'center')
    return 'text-center'
  if (column.align === 'right')
    return 'text-right'
  return 'text-left'
}

export function cellValue(row: TableRow, key: string) {
  return (row as Record<string, unknown>)[key]
}

function isEmptyCellValue(value: unknown) {
  return value === undefined || value === null || value === ''
}

export function cellDisplayValue<Row extends TableRow>(column: BaseTableColumn<Row>, row: Row) {
  const rawValue = cellValue(row, column.key)
  const value = column.format ? column.format(rawValue, row) : rawValue
  return isEmptyCellValue(value) ? (column.emptyText ?? '—') : value
}

export function cellTitle<Row extends TableRow>(column: ResolvedTableColumn<Row>, row: Row) {
  if (!column.truncate)
    return undefined
  const value = cellDisplayValue(column, row)
  return typeof value === 'string' || typeof value === 'number' ? String(value) : undefined
}

export function cellContentClass<Row extends TableRow>(column: ResolvedTableColumn<Row>) {
  if (column.kind === 'selection' || column.kind === 'expander') {
    return [
      'flex min-w-0 items-center overflow-visible leading-none',
      column.align === 'right'
        ? 'justify-end'
        : column.align === 'center'
          ? 'justify-center'
          : 'justify-start',
    ]
  }
  if (column.kind === 'actions')
    return 'min-w-0 overflow-visible'
  return ['min-w-0', column.truncate ? 'truncate' : undefined]
}

export function columnSortKey<Row extends TableRow>(column: BaseTableColumn<Row>) {
  return typeof column.sortable === 'string' ? column.sortable : column.key
}
