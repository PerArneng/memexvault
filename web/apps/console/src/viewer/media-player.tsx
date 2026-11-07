import { formatClock, playability } from "@memex/core";
import { Button } from "@memex/ui";
import { useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import type { Ref, SyntheticEvent } from "react";

export interface MediaPlayerHandle {
  seek(ms: number): void;
  /** Plays a range, and stops at its end if it has one. */
  playRange(fromMs: number, toMs: number | null): void;
}

export interface MediaPlayerProps {
  readonly ref?: Ref<MediaPlayerHandle>;
  readonly src: string;
  readonly mediaType: string;
  /** Where to start, and where to stop. Changing `fromMs` starts again from there. */
  readonly fromMs: number | null;
  readonly toMs: number | null;
  readonly downloadUrl: string;
  readonly onTime: (ms: number) => void;
}

const failureText = (error: MediaError | null): string => {
  switch (error?.code) {
    case 1:
      return "Loading was stopped.";
    case 2:
      return "The connection failed partway, or the server can't read this recording's file.";
    case 3:
      return "This browser couldn't decode the recording. The file may be damaged, or use a codec it doesn't have.";
    default:
      return "This browser can't play this type of recording, or the server couldn't send it.";
  }
};

/**
 * The browser's own player, on a same-origin address it reads with Range requests, so a long recording isn't downloaded to seek in it.
 * `src` needs no blob URL, which the CSP blocks, and the session cookie goes with it. Nothing here knows about sources.
 */
export const MediaPlayer = ({ ref, src, mediaType, fromMs, toMs, downloadUrl, onTime }: MediaPlayerProps) => {
  const element = useRef<HTMLMediaElement | null>(null);
  const stopAt = useRef<number | null>(toMs);
  const [blocked, setBlocked] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  // Asked of a spare element, so a type with no player never starts a download.
  const [cannotPlay] = useState(() => playability(document.createElement(mediaType.startsWith("video/") ? "video" : "audio").canPlayType(mediaType)) === "no");
  const pending = useRef<number | null>(fromMs);

  const start = useCallback((ms: number) => {
    const media = element.current;
    if (media === null) return;
    media.currentTime = ms / 1000;
    media.play().then(
      () => setBlocked(false),
      (e: unknown) => {
        // The browser wants a click first. It's at the right place, ready to play. An interrupted `play` needs no message.
        if (e instanceof DOMException && e.name === "NotAllowedError") setBlocked(true);
      },
    );
  }, []);

  useImperativeHandle(ref, () => ({
    seek: (ms) => {
      stopAt.current = null;
      if (element.current !== null) element.current.currentTime = ms / 1000;
    },
    playRange: (from, to) => {
      stopAt.current = to;
      start(from);
    },
  }), [start]);

  // A new link into the same recording, once its details have loaded.
  useEffect(() => {
    stopAt.current = toMs;
    pending.current = fromMs;
    const media = element.current;
    if (fromMs !== null && media !== null && media.readyState >= 1) {
      pending.current = null;
      start(fromMs);
    }
  }, [fromMs, start, toMs]);

  const onLoadedMetadata = () => {
    if (pending.current !== null) {
      const at = pending.current;
      pending.current = null;
      start(at);
    }
  };

  const onTimeUpdate = (e: SyntheticEvent<HTMLMediaElement>) => {
    const ms = e.currentTarget.currentTime * 1000;
    onTime(ms);
    if (stopAt.current !== null && ms >= stopAt.current) {
      stopAt.current = null;
      e.currentTarget.pause();
    }
  };

  const shared = {
    src,
    controls: true,
    preload: "metadata" as const,
    onLoadedMetadata,
    onTimeUpdate,
    onPlaying: () => setBlocked(false),
    onError: (e: SyntheticEvent<HTMLMediaElement>) => setFailed(failureText(e.currentTarget.error)),
  };

  return (
    <div className="flex flex-col gap-2">
      {failed === null && !cannotPlay ? (
        mediaType.startsWith("video/") ? (
          <video ref={(v) => { element.current = v; }} {...shared} className="max-h-72 w-full rounded-md bg-surface-raised"><track kind="captions" /></video>
        ) : (
          <audio ref={(a) => { element.current = a; }} {...shared} className="w-full" />
        )
      ) : (
        <div role="alert" data-media-problem={true} className="flex flex-col gap-1 rounded-lg border border-warning bg-warning-surface p-3">
          <p className="font-medium text-warning">This recording can't be played here.</p>
          <p className="text-sm text-text-muted">{failed ?? `This browser has no player for ${mediaType}.`} The transcript still works, and you can <a href={downloadUrl} className="text-accent underline">download the original</a> to play it elsewhere.</p>
        </div>
      )}
      {blocked && (
        <div role="status" className="flex items-center gap-2 rounded-lg border border-info bg-info-surface p-2 text-sm">
          <span>The browser wants a click before it plays sound. It's ready at {formatClock(fromMs ?? 0)}.</span>
          <Button onClick={() => void element.current?.play().then(() => setBlocked(false))}>Play</Button>
        </div>
      )}
    </div>
  );
};
