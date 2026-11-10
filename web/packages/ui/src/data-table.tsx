import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef } from "react";
import type { CSSProperties, ReactNode } from "react";

export interface Column<Row> {
  readonly id: string;
  readonly header: string;
  readonly width: string;
  readonly numeric?: boolean;
  readonly cell: (row: Row) => ReactNode;
  /** Replaces the header's text, such as with a button that sorts. `header` still names the column. */
  readonly headerCell?: ReactNode;
  readonly ariaSort?: "ascending" | "descending" | "none";
}

export interface DataTableProps<Row> {
  readonly label: string;
  readonly rows: readonly Row[];
  readonly columns: readonly Column<Row>[];
  readonly rowKey: (row: Row) => string;
  readonly height: number;
  readonly onRowOpen?: (row: Row) => void;
  /** Called when the last rows are close to view, so a long list can load more. */
  readonly onNearEnd?: () => void;
}

const ROW_HEIGHT = 30;
const NEAR_END_ROWS = 60;

/** Only visible rows are in the DOM. aria-rowcount tells assistive tech the real size. */
export const DataTable = <Row,>({ label, rows, columns, rowKey, height, onRowOpen, onNearEnd }: DataTableProps<Row>) => {
  const scroller = useRef<HTMLDivElement>(null);
  const virtual = useVirtualizer({ count: rows.length, getScrollElement: () => scroller.current, estimateSize: () => ROW_HEIGHT, overscan: 12 });
  const lastVisible = virtual.getVirtualItems().at(-1)?.index ?? -1;
  useEffect(() => {
    if (onNearEnd !== undefined && lastVisible >= rows.length - NEAR_END_ROWS) onNearEnd();
  }, [lastVisible, onNearEnd, rows.length]);
  const template: CSSProperties = { gridTemplateColumns: columns.map((c) => c.width).join(" ") };
  return (
    <div
      ref={scroller}
      role="table"
      aria-label={label}
      aria-rowcount={rows.length + 1}
      tabIndex={0}
      style={{ height }}
      className="overflow-auto rounded-md border border-border"
    >
      <div role="rowgroup" className="sticky top-0 z-10 bg-surface-raised">
        <div role="row" aria-rowindex={1} style={template} className="grid border-b border-border-strong">
          {columns.map((c) => (
            <div key={c.id} role="columnheader" {...(c.ariaSort === undefined ? {} : { "aria-sort": c.ariaSort })} className={`px-2 py-1.5 text-xs font-medium text-text-muted ${c.numeric === true ? "text-right" : ""}`}>
              {c.headerCell ?? c.header}
            </div>
          ))}
        </div>
      </div>
      <div role="rowgroup" style={{ height: virtual.getTotalSize() }} className="relative">
        {virtual.getVirtualItems().map((item) => {
          const row = rows[item.index];
          if (row === undefined) return null;
          return (
            <div
              key={rowKey(row)}
              role="row"
              aria-rowindex={item.index + 2}
              onDoubleClick={onRowOpen === undefined ? undefined : () => onRowOpen(row)}
              style={{ ...template, height: item.size, transform: `translateY(${item.start}px)` }}
              className="absolute top-0 left-0 grid w-full items-center border-b border-border hover:bg-surface-raised"
            >
              {columns.map((c) => (
                <div key={c.id} role="cell" className={`truncate px-2 text-sm ${c.numeric === true ? "text-right font-mono text-xs tabular-nums" : ""}`}>
                  {c.cell(row)}
                </div>
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
};
