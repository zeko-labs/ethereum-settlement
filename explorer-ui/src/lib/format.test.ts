import { describe, expect, it } from "vitest";
import {
  formatInteger,
  formatNano,
  formatTimestamp,
  formatWei,
  timeAgo,
} from "./format";

describe("lossless amount formatting", () => {
  it("never converts uint64 identifiers through JavaScript numbers", () => {
    expect(formatInteger("18446744073709551615")).toBe(
      "18,446,744,073,709,551,615",
    );
    expect(formatNano("18446744073709551615")).toBe(
      "18,446,744,073.709551 ZEKO",
    );
  });

  it("formats wei from its decimal string", () => {
    expect(formatWei("24820000000000000000")).toBe("24.82 ETH");
  });

  it("reads the archive's Unix-millisecond timestamp strings", () => {
    expect(timeAgo("1721048400000", 1721048412000)).toBe("12 sec");
    expect(formatTimestamp("not-a-date")).toBe("not-a-date");
  });
});
