import type { ReactNode } from "react";
import { compact, sentence } from "../lib/format";

export function Icon({
  name,
}: {
  name: "search" | "refresh" | "arrow" | "chevron" | "block" | "copy";
}) {
  const icons = {
    search: "⌕",
    refresh: "↻",
    arrow: "↗",
    chevron: "›",
    block: "▦",
    copy: "□",
  };
  return (
    <span className={`glyph glyph-${name}`} aria-hidden="true">
      {icons[name]}
    </span>
  );
}

export function Status({ children }: { children: string }) {
  const label = sentence(children);
  const tone = children.toLowerCase().replaceAll(" ", "-");
  return (
    <span className={`status status-${tone}`}>
      <span className="status-dot" />
      {label}
    </span>
  );
}

export function Address({
  children,
  copy = false,
}: {
  children: string | null | undefined;
  copy?: boolean;
}) {
  const value = children ?? "—";
  return (
    <span className="address-wrap">
      <span className="mono address" title={value}>
        {value}
      </span>
      {copy && children ? (
        <button
          className="copy-button"
          aria-label="Copy identifier"
          onClick={() => void navigator.clipboard?.writeText(children)}
        >
          <Icon name="copy" />
        </button>
      ) : null}
    </span>
  );
}

export function ShortAddress({ value }: { value: string | null | undefined }) {
  return (
    <span className="mono" title={value ?? undefined}>
      {compact(value)}
    </span>
  );
}

export function SectionHeading({
  eyebrow,
  title,
  action,
}: {
  eyebrow: string;
  title: string;
  action?: ReactNode;
}) {
  return (
    <div className="section-heading">
      <div>
        <span className="section-eyebrow">{eyebrow}</span>
        <h2>{title}</h2>
      </div>
      {action}
    </div>
  );
}

export function LoadingRows({ count = 4 }: { count?: number }) {
  return (
    <div className="loading-rows" aria-label="Loading explorer records">
      {Array.from({ length: count }, (_, index) => (
        <div className="loading-row" key={index}>
          <span />
          <span />
          <span />
        </div>
      ))}
    </div>
  );
}

export function EmptyState({ title, copy }: { title: string; copy: string }) {
  return (
    <div className="empty-state">
      <span className="empty-emblem">Z</span>
      <strong>{title}</strong>
      <p>{copy}</p>
    </div>
  );
}

export function ErrorState({
  message,
  retry,
}: {
  message: string;
  retry?: () => void;
}) {
  return (
    <div className="error-state" role="alert">
      <div>
        <strong>Explorer data unavailable</strong>
        <p>{message}</p>
      </div>
      {retry ? <button onClick={retry}>Try again</button> : null}
    </div>
  );
}

export function RefreshButton({
  onClick,
  loading,
  label = true,
}: {
  onClick: () => void;
  loading: boolean;
  label?: boolean;
}) {
  return (
    <button
      className={`refresh-button${loading ? " spinning" : ""}`}
      onClick={onClick}
      aria-label="Refresh data"
    >
      <Icon name="refresh" />
      {label ? " Refresh" : null}
    </button>
  );
}

export function Updated({ updatedAt }: { updatedAt: number | null }) {
  return (
    <span className="list-meta">
      <span className="live-dot" />
      {updatedAt
        ? `Live · updated ${Math.max(0, Math.round((Date.now() - updatedAt) / 1000))} sec ago`
        : "Connecting to indexers"}
    </span>
  );
}

export function DetailHero({
  eyebrow,
  title,
  copy,
  status,
}: {
  eyebrow: string;
  title: string;
  copy?: string;
  status?: string;
}) {
  return (
    <div className="detail-hero">
      <div>
        <span className="hero-kicker">{eyebrow}</span>
        <h1>{title}</h1>
        {copy ? <p>{copy}</p> : null}
      </div>
      {status ? <Status>{status}</Status> : null}
    </div>
  );
}

export function DetailGrid({ children }: { children: ReactNode }) {
  return <div className="detail-grid page-detail-grid">{children}</div>;
}

export function Field({
  label,
  children,
  wide = false,
}: {
  label: string;
  children: ReactNode;
  wide?: boolean;
}) {
  return (
    <div className={wide ? "wide" : undefined}>
      <span>{label}</span>
      <strong>{children}</strong>
    </div>
  );
}

export function Pager({
  hasPrevious,
  hasNext,
  onPrevious,
  onNext,
}: {
  hasPrevious: boolean;
  hasNext: boolean;
  onPrevious: () => void;
  onNext: () => void;
}) {
  return (
    <div className="pagination">
      <button disabled={!hasPrevious} onClick={onPrevious}>
        Previous
      </button>
      <span>Newest records first</span>
      <button disabled={!hasNext} onClick={onNext}>
        Next
      </button>
    </div>
  );
}
