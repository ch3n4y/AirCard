import { expect, test, type Page } from "@playwright/test";

async function mockDevice(page: Page, slowRead = false) {
  await page.addInitScript(
    ({ slowRead }) => {
      const cards = [
        {
          hash: "first+card/hash=",
          has_original: true,
          has_artwork: true,
          seen_in_last_scan: true,
        },
        {
          hash: "second+card/hash=",
          has_original: false,
          has_artwork: true,
          seen_in_last_scan: true,
        },
        {
          hash: "third+card/hash=",
          has_original: true,
          has_artwork: false,
          seen_in_last_scan: true,
        },
        {
          hash: "fourth+card/hash=",
          has_original: true,
          has_artwork: false,
          seen_in_last_scan: true,
        },
      ];
      const art = (color: string) =>
        "data:image/svg+xml," +
        encodeURIComponent(
          '<svg xmlns="http://www.w3.org/2000/svg" width="400" height="252"><defs><linearGradient id="g" x2="1" y2="1"><stop stop-color="' +
            color +
            '"/><stop offset="1" stop-color="#182847"/></linearGradient></defs><rect width="400" height="252" rx="20" fill="url(#g)"/><circle cx="335" cy="45" r="115" fill="white" opacity=".08"/><circle cx="340" cy="60" r="150" fill="none" stroke="white" opacity=".12"/><rect x="30" y="90" width="45" height="34" rx="7" fill="#efdeb2"/><text x="30" y="42" fill="white" font-family="sans-serif" font-size="18">AirCard</text><text x="30" y="204" fill="white" opacity=".75" font-family="sans-serif" font-size="14">••••  ••••  ••••  2026</text></svg>',
        );
      const w = window as any;
      w.__calls = [];
      w.__finishRead = () => {};
      w.__TAURI_INTERNALS__ = {
        invoke: async (command: string, args: any) => {
          w.__calls.push({ command, args });
          switch (command) {
            case "list_devices":
              return [
                {
                  udid: "test-phone",
                  name: "我的 iPhone",
                  product: "iPhone",
                  version: "18.6",
                  build: "test",
                  language: "zh",
                  locale: "zh_CN",
                  connection: "usb",
                  bold_text: false,
                },
              ];
            case "app_paths":
              return {};
            case "cards":
            case "fold_scan":
              return cards;
            case "card_thumbnail":
              return art(args.hash.startsWith("first") ? "#459b8f" : "#727ac8");
            case "plugin:dialog|open":
              return "/mock/new-skin.png";
            case "image_preview":
              return art("#8f78ca");
            case "flash_cards":
              return args.hashes.map((card: string) => ({
                card,
                ok: true,
                message: "完成",
              }));
            case "start_scan":
              return null;
            case "scan_status":
              return {
                running: false,
                found: cards.map((c) => c.hash),
                lines_read: 12,
              };
            case "read_artwork":
              if (slowRead) {
                await new Promise<void>((resolve) => {
                  w.__finishRead = resolve;
                });
              }
              return null;
            case "log_tail":
              return "";
            default:
              return null;
          }
        },
      };
    },
    { slowRead },
  );
}

test("select, deselect and flash the exact targets on one page", async ({
  page,
}) => {
  await mockDevice(page);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  const first = page.getByRole("button", { name: "选择卡片 01", exact: true });
  const second = page.getByRole("button", { name: "选择卡片 02", exact: true });
  await expect(first).toBeVisible();
  await expect(
    page.getByRole("button", { name: "刷入 iPhone", exact: true }),
  ).toBeDisabled();
  await first.click();
  await second.click();
  await first.click();
  await expect(first).toHaveAttribute("aria-pressed", "false");
  await expect(second).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("selected-count")).toHaveText("1 张卡片");
  await page.getByRole("button", { name: "选择图片", exact: true }).click();
  await expect(page.getByAltText("待刷入的卡面")).toBeVisible();
  await page.getByRole("button", { name: "刷入 iPhone", exact: true }).click();
  await expect(page.getByText("刷入完成", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() =>
    (window as any).__calls.filter((c: any) => c.command === "flash_cards"),
  );
  expect(calls).toEqual([
    {
      command: "flash_cards",
      args: {
        udid: "test-phone",
        hashes: ["second+card/hash="],
        image: "/mock/new-skin.png",
      },
    },
  ]);
  expect(errors).toEqual([]);
});

test("all selection, card menu and maintenance keep the targets consistent", async ({
  page,
}) => {
  await mockDevice(page);
  await page.goto("/");
  await page.getByRole("checkbox", { name: "全选", exact: true }).check();
  await expect(page.getByTestId("selected-count")).toHaveText("4 张卡片");
  await page
    .getByRole("button", { name: "卡片 01 的更多操作", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "恢复原始图像" }),
  ).toBeVisible();
  await expect(page.getByTestId("selected-count")).toHaveText("4 张卡片");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "维护", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.getByRole("button", { name: "关闭", exact: true }).click();
  await page.getByRole("checkbox", { name: "全选", exact: true }).uncheck();
  await expect(page.getByTestId("selected-count")).toHaveText("0 张卡片");
});

test("background artwork reading permits target selection but prevents flashing", async ({
  page,
}) => {
  await mockDevice(page, true);
  await page.goto("/");
  await page.getByRole("button", { name: "扫描卡片", exact: true }).click();
  await expect(page.getByText(/正在读取卡面 0\/2/)).toBeVisible();
  await page.getByRole("button", { name: "选择卡片 02", exact: true }).click();
  await page.getByRole("button", { name: "选择图片", exact: true }).click();
  await expect(page.getByTestId("selected-count")).toHaveText("1 张卡片");
  await expect(
    page.getByRole("button", { name: "刷入 iPhone", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "停止读取", exact: true }).click();
  await page.evaluate(() => (window as any).__finishRead());
  await expect(
    page.getByRole("button", { name: "刷入 iPhone", exact: true }),
  ).toBeEnabled();
});

test("single-page layout fits the minimum window and produces a preview", async ({
  page,
}) => {
  await mockDevice(page);
  await page.goto("/");
  await page.getByRole("button", { name: "选择卡片 01", exact: true }).click();
  await page.getByRole("button", { name: "选择卡片 02", exact: true }).click();
  await page.getByRole("button", { name: "选择图片", exact: true }).click();
  await expect(page.getByAltText("待刷入的卡面")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "刷入 iPhone", exact: true }),
  ).toBeEnabled();
  await page.screenshot({
    path: "test-results/single-page.png",
    fullPage: true,
  });
  await page.setViewportSize({ width: 980, height: 680 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await expect(
    page.getByRole("button", { name: "刷入 iPhone", exact: true }),
  ).toBeInViewport();
  await page.screenshot({
    path: "test-results/minimum-window.png",
    fullPage: true,
  });
});
