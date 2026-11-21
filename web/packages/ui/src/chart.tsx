// The design system's one chart style (spec 047): stacked columns on a faint grid, series in the chart tokens' order, a legend
// above, and no numbers in the drawing. Exact numbers belong in a table beside it, so the chart is hidden from screen readers
// apart from its label, which says where the table is.

const SERIES_FILL = ["fill-chart-1", "fill-chart-2", "fill-chart-3", "fill-chart-4", "fill-chart-5"] as const;
const SERIES_BG = ["bg-chart-1", "bg-chart-2", "bg-chart-3", "bg-chart-4", "bg-chart-5"] as const;

export interface ChartColumn {
  /** Shown under the column, and in its tooltip. */
  readonly label: string;
  /** One value a series, in the order of `series`. */
  readonly values: readonly number[];
}

export interface StackedColumnChartProps {
  /** Says what the chart shows and where its exact numbers are. */
  readonly label: string;
  readonly series: readonly string[];
  readonly columns: readonly ChartColumn[];
  /** Label every nth column, so a month's days don't crowd. */
  readonly labelEvery?: number;
  readonly unit?: string;
}

const WIDTH = 720;
const HEIGHT = 200;
const AXIS = 18;

/** A round number at or above the largest stack, for the grid's top. */
const niceTop = (max: number): number => {
  if (max <= 0) return 1;
  const step = 10 ** Math.floor(Math.log10(max));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * step >= max) return m * step;
  return 10 * step;
};

const short = (n: number): string => (n >= 1_000_000 ? `${(n / 1_000_000).toFixed(n % 1_000_000 === 0 ? 0 : 1)}M` : n >= 1000 ? `${(n / 1000).toFixed(n % 1000 === 0 ? 0 : 1)}k` : String(n));

export const StackedColumnChart = ({ label, series, columns, labelEvery = 1, unit = "" }: StackedColumnChartProps) => {
  const top = niceTop(Math.max(0, ...columns.map((c) => c.values.reduce((a, b) => a + b, 0))));
  const plot = HEIGHT - AXIS;
  const slot = columns.length === 0 ? WIDTH : WIDTH / columns.length;
  const bar = Math.max(1, slot * 0.7);
  const y = (v: number) => plot - (v / top) * plot;
  return (
    <figure className="flex flex-col gap-2" aria-label={label}>
      <ul className="flex flex-wrap gap-3 text-xs text-text-muted" aria-hidden="true">
        {series.map((s, i) => (
          <li key={s} className="flex items-center gap-1">
            <span className={`inline-block size-2.5 rounded-sm ${SERIES_BG[i % SERIES_BG.length] ?? ""}`} />
            {s}
          </li>
        ))}
      </ul>
      <div className="flex gap-2">
        <div className="flex flex-col justify-between pb-[18px] text-right font-mono text-xs text-text-faint tabular-nums" aria-hidden="true">
          <span>{short(top)}{unit}</span>
          <span>{short(top / 2)}{unit}</span>
          <span>0</span>
        </div>
        <svg viewBox={`0 0 ${String(WIDTH)} ${String(HEIGHT)}`} className="h-48 w-full" role="img" aria-label={label} preserveAspectRatio="none">
          {[0, 0.5, 1].map((f) => <line key={f} x1={0} x2={WIDTH} y1={y(top * f)} y2={y(top * f)} className="stroke-chart-grid" strokeWidth={1} vectorEffect="non-scaling-stroke" />)}
          {columns.map((c, ci) => {
            let base = 0;
            const x = ci * slot + (slot - bar) / 2;
            return (
              <g key={c.label}>
                <title>{`${c.label}: ${c.values.map((v, si) => `${series[si] ?? ""} ${String(v)}`).join(", ")}`}</title>
                {c.values.map((v, si) => {
                  if (v <= 0) return null;
                  const from = base;
                  base += v;
                  return <rect key={series[si] ?? si} x={x} width={bar} y={y(base)} height={y(from) - y(base)} className={SERIES_FILL[si % SERIES_FILL.length]} />;
                })}
              </g>
            );
          })}
        </svg>
      </div>
      <div className="flex justify-between pl-8 font-mono text-xs text-text-faint" aria-hidden="true">
        {columns.filter((_, i) => i % labelEvery === 0).map((c) => <span key={c.label}>{c.label}</span>)}
      </div>
    </figure>
  );
};
