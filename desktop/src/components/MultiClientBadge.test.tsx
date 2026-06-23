// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { MultiClientBadge } from "./MultiClientBadge";

afterEach(() => {
  cleanup();
});

describe("MultiClientBadge", () => {
  it("renders nothing for a single (or zero) attached client", () => {
    const { container, rerender } = render(<MultiClientBadge count={1} />);
    expect(container.firstChild).toBeNull();
    rerender(<MultiClientBadge count={0} />);
    expect(container.firstChild).toBeNull();
  });

  it("renders a labelled mark when more than one client is attached", () => {
    render(<MultiClientBadge count={3} />);
    const badge = screen.getByRole("img");
    // Leads with a separator so it suffixes the parent button's name as
    // "host, 3 viewers" instead of gluing onto it; the caveat rides the tooltip.
    expect(badge.getAttribute("aria-label")).toBe(", 3 viewers");
    expect(badge.getAttribute("title")).toContain("3 clients attached to this server");
  });
});
