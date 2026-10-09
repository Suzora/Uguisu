import { describe, expect, it } from "vitest";
import {
  bytes,
  date,
  dateTime,
  duration,
  eta,
  percentage,
  rate,
  relative,
} from "./format";

describe("formatting", () => {
  it("scales byte counts", () => {
    expect(bytes(0)).toBe("0 B");
    expect(bytes(1023)).toBe("1023 B");
    expect(bytes(1024)).toBe("1.0 KiB");
    expect(bytes(48_000_000)).toBe("45.8 MiB");
    expect(bytes(null)).toBe("—");
  });

  it("renders durations as clocks", () => {
    expect(duration(59)).toBe("0:59");
    expect(duration(3723)).toBe("1:02:03");
    expect(duration(null)).toBe("—");
  });

  it("shows an unknown rate as a dash", () => {
    expect(rate(1_500_000)).toBe("1.4 MiB/s");
    expect(rate(null)).toBe("—");
    expect(rate(0)).toBe("—");
  });

  it("counts down in useful units", () => {
    expect(eta(30)).toBe("30 s left");
    expect(eta(600)).toBe("10 min left");
    expect(eta(null)).toBe("—");
  });

  it("withholds a percentage without a total", () => {
    expect(percentage(50, 200)).toBe(25);
    expect(percentage(1, null)).toBeNull();
    expect(percentage(1, 0)).toBeNull();
  });

  it("places a time on either side of now", () => {
    const now = Date.parse("2026-01-01T12:00:00Z");
    expect(relative("2026-01-01T11:58:00Z", now)).toBe("2 minutes ago");
    expect(relative("2026-01-01T12:30:00Z", now)).toBe("in 30 minutes");
    expect(relative("2025-12-30T12:00:00Z", now)).toBe("2 days ago");
    expect(relative(null)).toBe("—");
  });

  it("dates in the interface language", () => {
    // Midday UTC is the same calendar day in every time zone a runner uses.
    expect(date("2026-01-15T12:00:00Z")).toBe("Jan 15, 2026");
    expect(dateTime("2026-01-15T12:00:00Z")).toMatch(
      /^Jan 15, 2026, \d{1,2}:00:00\s[AP]M$/,
    );
    expect(date(null)).toBe("—");
  });
});
