#!/bin/bash
# Spec 035: each temporary violation must fail its check, then is reverted. Run from web/.
try() { # name file line check
  local name="$1" file="$2" line="$3" check="$4"
  local had=0; [ -f "$file" ] && had=1
  [ $had = 1 ] && cp "$file" "$file.bak"
  printf '%s\n' "$line" >> "$file"
  if out=$(eval "$check" 2>&1); then r="NOT CAUGHT"; else r="caught"; fi
  [ $had = 1 ] && mv "$file.bak" "$file" || rm "$file"
  echo "== $name: $r"; echo "$out" | grep -E "error |✘|TS[0-9]+|denied:|\berror\b" | grep -v "node version" | head -2 | cut -c1-170
}
C=packages/core/src/probe.ts; V=apps/console/src/view-probe.tsx
TC='pnpm exec tsc --noEmit -p packages/core/tsconfig.json'
ES="pnpm exec eslint packages apps"; DC='pnpm exec depcruise packages apps --config .dependency-cruiser.cjs'
try "fetch in core (typecheck)" $C 'export const f = () => fetch("/x");' "$TC"
try "window in core (typecheck)" $C 'export const w = window;' "$TC"
try "React import in core (typecheck)" $C 'import { useState } from "react"; export const u = useState;' "$TC"
try "Date.now in core (lint)" $C 'export const n = () => Date.now();' "$ES"
try "new Date in core (lint)" $C 'export const n = () => new Date();' "$ES"
try "Math.random in core (lint)" $C 'export const n = () => Math.random();' "$ES"
try "setTimeout in core (lint)" $C 'export const n = () => setTimeout(() => 1, 1);' "$ES"
try "any in core (lint)" $C 'export const n = (a: any) => a;' "$ES"
try "dangerouslySetInnerHTML (lint)" $V 'export const P = () => <div dangerouslySetInnerHTML={{ __html: "x" }} />;' "$ES"
try "edge-http into core (depcruise)" $C 'import "../../edge-http/src/index";' "$DC"
try "view into edge-http (depcruise)" $V 'import "../../../packages/edge-http/src/index";' "$DC"
try "core into react by path (depcruise)" $C 'import "react";' "$DC"
try "denied licence fixture" tools/.probe '' 'node tools/check-licenses.mjs ../specs/todo/035-web-workspace/tools/denied-fixture.json'
