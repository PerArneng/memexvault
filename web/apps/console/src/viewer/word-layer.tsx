import { wordPlacement } from "@memex/core";
import type { PageWord } from "@memex/types";
import { useMemo } from "react";

/** A page image has no text, so this lays the word table's words over it, invisible but selectable. */
const MEASURE_PX = 100;
const FONT_FAMILY = "sans-serif";
/** Glyphs fill about this much of a line's height. */
const FONT_TO_BOX = 0.85;

export const WordLayer = ({ words, angle, zoom }: { readonly words: readonly PageWord[]; readonly angle: number; readonly zoom: number }) => {
  // A canvas measures text with no layout, and the CSP has no say over it.
  const context = useMemo(() => document.createElement("canvas").getContext("2d"), []);
  if (context !== null) context.font = `${String(MEASURE_PX)}px ${FONT_FAMILY}`;
  return (
    <div data-word-layer={true} className="absolute inset-0 overflow-hidden text-transparent selection:bg-accent/40">
      {words.map((word, index) => {
        const placed = wordPlacement(word.rect, angle, word.quad, zoom);
        const fontSize = placed.thickness * FONT_TO_BOX;
        const natural = context === null ? 0 : (context.measureText(word.text).width * fontSize) / MEASURE_PX;
        const stretch = natural > 0 ? placed.length / natural : 1;
        return (
          // React sets these through the CSSOM, which the CSP allows. Text only, so anything in a word stays text.
          // The trailing space is what separates words when they're copied. A space between spans would collapse, and the fixed width keeps this one out of the box.
          <span
            key={index}
            data-word={word.text}
            className="absolute origin-top-left whitespace-pre text-transparent"
            style={{
              left: placed.left,
              top: placed.top,
              width: natural > 0 ? natural : undefined,
              fontFamily: FONT_FAMILY,
              fontSize,
              lineHeight: `${String(placed.thickness)}px`,
              transform: `rotate(${String(placed.angle)}deg) scaleX(${String(stretch)})`,
            }}
          >
            {`${word.text} `}
          </span>
        );
      })}
    </div>
  );
};
