import { cssColor, parseAnsi, stripAnsi } from "./ansi";

describe("parseAnsi", () => {
  it("returns one plain segment for text without escapes", () => {
    expect(parseAnsi("hello world")).toEqual([{ text: "hello world", style: {} }]);
  });

  it("splits text at colour changes and resets", () => {
    expect(parseAnsi("a \x1b[31mred\x1b[0m b")).toEqual([
      { text: "a ", style: {} },
      { text: "red", style: { fg: 1 } },
      { text: " b", style: {} },
    ]);
  });

  it("reads bold, bright, background, 256-colour and truecolour codes", () => {
    const [bold, bright, bg, c256, rgb] = parseAnsi(
      "\x1b[1mB\x1b[0;92mG\x1b[0;44mU\x1b[0;38;5;208mO\x1b[0;38;2;255;0;128mP",
    );
    expect(bold.style).toEqual({ bold: true });
    expect(bright.style).toEqual({ fg: 10 });
    expect(bg.style).toEqual({ bg: 4 });
    expect(c256.style).toEqual({ fg: 208 });
    expect(rgb.style).toEqual({ fg: "#ff0080" });
  });

  it("keeps styles that later codes do not change", () => {
    expect(parseAnsi("\x1b[1;31mx\x1b[39my")).toEqual([
      { text: "x", style: { bold: true, fg: 1 } },
      { text: "y", style: { bold: true } },
    ]);
  });

  it("treats an empty parameter list as a reset", () => {
    expect(parseAnsi("\x1b[31mx\x1b[my")[1]).toEqual({ text: "y", style: {} });
  });

  it("drops cursor movement, OSC sequences and control characters", () => {
    expect(parseAnsi("\x1b[2K\x1b]0;title\x07a\x07b\x1b[1Gc")).toEqual([{ text: "abc", style: {} }]);
  });

  it("ignores a truncated extended colour", () => {
    expect(parseAnsi("\x1b[38;2;10mx")).toEqual([{ text: "x", style: {} }]);
  });
});

describe("stripAnsi", () => {
  it("returns the text a reader sees", () => {
    expect(stripAnsi("\x1b[1;32mINFO\x1b[0m started")).toBe("INFO started");
    expect(stripAnsi("plain\ttab")).toBe("plain\ttab");
  });
});

describe("cssColor", () => {
  it("uses theme variables for the 16 basic colours and computes the rest", () => {
    expect(cssColor(1)).toBe("var(--ansi-1)");
    expect(cssColor(196)).toBe("rgb(255, 0, 0)");
    expect(cssColor(232)).toBe("rgb(8, 8, 8)");
    expect(cssColor("#123456")).toBe("#123456");
  });
});
