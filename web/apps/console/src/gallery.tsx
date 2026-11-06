import {
  Badge, Button, CommandPalette, ConflictBadge, DataTable, Dialog, EmptyState, EpistemicBadge, ErrorState, Input, Menu, Select, Skeleton, SplitPane, StageBadge, Tabs, Timeline, Toasts, Tooltip, TrustBadge,
} from "@memex/ui";
import type { Column, ToastMessage } from "@memex/ui";
import { useMemo, useState } from "react";
import type { ReactNode } from "react";

interface Row { readonly id: string; readonly name: string; readonly stage: string; readonly pages: number }

const COLUMNS: readonly Column<Row>[] = [
  { id: "name", header: "Document", width: "2fr", cell: (r) => r.name },
  { id: "stage", header: "Stage", width: "1fr", cell: (r) => r.stage },
  { id: "pages", header: "Pages", width: "6rem", numeric: true, cell: (r) => r.pages },
];

const Section = ({ title, children }: { readonly title: string; readonly children: ReactNode }) => (
  <section aria-label={title} className="flex flex-col gap-2">
    <h2 className="text-lg font-medium">{title}</h2>
    <div className="flex flex-wrap items-start gap-2">{children}</div>
  </section>
);

/** Every component and state, for review and tests. No data access. */
export const Gallery = () => {
  const [dialog, setDialog] = useState(false);
  const [palette, setPalette] = useState(false);
  const [lang, setLang] = useState("sv");
  const [ran, setRan] = useState("nothing yet");
  const [toasts, setToasts] = useState<readonly ToastMessage[]>([]);
  const rows = useMemo<readonly Row[]>(() => Array.from({ length: 100_000 }, (_, i) => ({ id: String(i), name: `document-${i}.pdf`, stage: i % 7 === 0 ? "OCR" : "Done", pages: i % 400 })), []);
  return (
    <main className="mx-auto flex max-w-6xl flex-col gap-6 p-6">
      <h1 className="text-2xl font-medium">Design system</h1>
      <Section title="Buttons">
        <Button variant="primary">Approve</Button>
        <Button>Open</Button>
        <Button variant="danger">Reject</Button>
        <Button variant="ghost">Cancel</Button>
        <Button disabled>Disabled</Button>
      </Section>
      <Section title="Fields">
        <Input label="Search vault" placeholder="Ask or filter" />
        <Input label="Token name" defaultValue="too long" error="Use at most 4 characters" />
        <Select label="Language" value={lang} onValueChange={setLang} options={[{ value: "en", label: "English" }, { value: "sv", label: "Swedish" }, { value: "de", label: "German" }, { value: "es", label: "Spanish" }]} />
      </Section>
      <Section title="Overlays">
        <Button onClick={() => setDialog(true)}>Open dialog</Button>
        <Dialog open={dialog} onOpenChange={setDialog} title="Reject proposal" description="This can be undone from the ChangeSet.">
          <Button variant="danger" onClick={() => setDialog(false)}>Reject</Button>
        </Dialog>
        <Menu trigger={<Button>Open menu</Button>} items={[{ id: "a", label: "Reprocess", onSelect: () => setRan("Reprocess") }, { id: "b", label: "Download original", onSelect: () => setRan("Download") }]} />
        <Tooltip text="Shows provenance"><Button>Tooltip</Button></Tooltip>
        <Button onClick={() => setToasts((t) => [...t, { id: String(t.length), title: "Upload finished", detail: "report.pdf, 42 pages" }])}>Show toast</Button>
        <Button onClick={() => setPalette(true)}>Open palette</Button>
        <CommandPalette open={palette} onOpenChange={setPalette} commands={[{ id: "search", label: "Go to search", hint: "g s", run: () => setRan("Go to search") }, { id: "review", label: "Go to review queue", hint: "g r", run: () => setRan("Go to review queue") }, { id: "upload", label: "Upload document", run: () => setRan("Upload document") }]} />
        <span data-testid="ran" className="text-sm text-text-muted">Last action: {ran}</span>
        <Toasts messages={toasts} onDismiss={(id) => setToasts((t) => t.filter((m) => m.id !== id))} />
      </Section>
      <Section title="Badges">
        <Badge>Neutral</Badge><Badge tone="info">Info</Badge><Badge tone="success">Success</Badge><Badge tone="warning">Warning</Badge><Badge tone="danger">Danger</Badge>
        <TrustBadge tier="verified" /><TrustBadge tier="trusted" /><TrustBadge tier="unverified" /><TrustBadge tier="untrusted" />
        <EpistemicBadge status="asserted" /><EpistemicBadge status="inferred" /><EpistemicBadge status="disputed" /><EpistemicBadge status="superseded" />
        <StageBadge stage="waiting" /><StageBadge stage="running" detail="OCR page 12 of 400" /><StageBadge stage="partial" /><StageBadge stage="failed" /><StageBadge stage="done" />
        <ConflictBadge count={1} /><ConflictBadge count={3} />
      </Section>
      <Section title="Tabs">
        <div className="w-full"><Tabs label="Document" tabs={[{ id: "pages", label: "Pages", content: "Page images and highlights." }, { id: "claims", label: "Claims", content: "Claims from this document." }, { id: "versions", label: "Versions", content: "Earlier versions." }]} /></div>
      </Section>
      <Section title="States">
        <div className="flex w-64 flex-col gap-2"><Skeleton className="h-4 w-full" /><Skeleton className="h-4 w-2/3" /></div>
        <div className="w-72"><EmptyState title="No documents yet">Upload a PDF to start.</EmptyState></div>
        <div className="w-96"><ErrorState title="Search failed" detail="503 from /api/v1/search" onRetry={() => setRan("Retry")} /></div>
      </Section>
      <Section title="Timeline">
        <div className="w-96"><Timeline items={[{ id: "1", title: "Stored", at: "12:00:01", tone: "success" }, { id: "2", title: "OCR", detail: "Page 12 of 400", at: "12:00:09", tone: "info" }, { id: "3", title: "Extraction waiting", detail: "LLM endpoint unreachable, retrying", at: "12:01:40", tone: "warning" }, { id: "4", title: "Failed", at: "12:02:00", tone: "danger" }]} /></div>
      </Section>
      <Section title="Split pane">
        <div className="h-32 w-full"><SplitPane label="Resize panes" start="Document page" end="Claims" /></div>
      </Section>
      <Section title="Data table">
        <div className="w-full"><DataTable label="Documents" rows={rows} columns={COLUMNS} rowKey={(r) => r.id} height={320} /></div>
      </Section>
    </main>
  );
};
