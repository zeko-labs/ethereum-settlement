module.exports = {
  preset: "@metamask/snaps-jest",
  testMatch: ["<rootDir>/src/**/*.test.ts"],
  transform: {
    "^.+\\.(t|j)sx?$": "ts-jest"
  }
}
