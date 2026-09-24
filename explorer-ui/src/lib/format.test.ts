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
      "18,446,744,073.709551615 ETH",
    );
  });

  it("uses the Zeko ledger scale for ETH balances and the Ethereum scale for wei", () => {
    expect(formatNano("1000000000")).toBe("1 ETH");
    expect(formatNano("2500")).toBe("0.0000025 ETH");
    expect(formatWei("1000000000000000000")).toBe("1 ETH");
  });

  it("formats wei from its decimal string", () => {
    expect(formatWei("24820000000000000000")).toBe("24.82 ETH");
  });

  it("reads the archive's Unix-millisecond timestamp strings", () => {
    expect(timeAgo("1721048400000", 1721048412000)).toBe("12 sec");
    expect(formatTimestamp("not-a-date")).toBe("not-a-date");
  });
});
