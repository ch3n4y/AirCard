import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { App as AntApp, Card, ConfigProvider, Empty, Flex, List, Typography } from "antd";

type AppPaths = { backups: string; artwork_cache: string; log_file: string };

/**
 * First screen: what this Mac already holds.
 *
 * The saved originals were written by the previous implementation, in the same
 * layout, so this doubles as proof that the Rust core reads that data unchanged.
 * The device layer (discovery, scanning, flashing) lands next.
 */
function AirCard() {
  const [paths, setPaths] = useState<AppPaths | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [originals, setOriginals] = useState<Record<string, string[]>>({});
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      try {
        const found = await invoke<string[]>("devices_with_saved_originals");
        setDevices(found);

        const byDevice: Record<string, string[]> = {};
        for (const udid of found) {
          byDevice[udid] = await invoke<string[]>("saved_originals", { udid });
        }
        setOriginals(byDevice);
        setPaths(await invoke<AppPaths>("app_paths"));
      } catch (error) {
        setProblem(String(error));
      }
    })();
  }, []);

  return (
    <ConfigProvider>
      <AntApp>
        <Flex vertical gap={16} style={{ padding: 24, maxWidth: 880, margin: "0 auto" }}>
          <Typography.Title level={3} style={{ marginBottom: 0 }}>
            AirCard
          </Typography.Title>
          <Typography.Text type="secondary">
            Replacement artwork for Apple Wallet cards. Originals already saved on this Mac are
            listed below.
          </Typography.Text>

          {problem && <Typography.Text type="danger">{problem}</Typography.Text>}

          {devices.length === 0 ? (
            <Empty description="No saved originals on this Mac yet" />
          ) : (
            devices.map((udid) => (
              <Card
                key={udid}
                size="small"
                title={<Typography.Text code>{udid}</Typography.Text>}
                extra={<Typography.Text type="secondary">{originals[udid]?.length ?? 0} cards</Typography.Text>}
              >
                <List
                  size="small"
                  dataSource={originals[udid] ?? []}
                  renderItem={(card) => (
                    <List.Item>
                      <Typography.Text code>{card}</Typography.Text>
                    </List.Item>
                  )}
                />
              </Card>
            ))
          )}

          {paths && (
            <Card size="small" title="On this Mac">
              <Flex vertical gap={2}>
                <Typography.Text type="secondary">
                  Saved originals: <Typography.Text code>{paths.backups}</Typography.Text>
                </Typography.Text>
                <Typography.Text type="secondary">
                  Artwork read from the phone:{" "}
                  <Typography.Text code>{paths.artwork_cache}</Typography.Text>
                </Typography.Text>
                <Typography.Text type="secondary">
                  Log: <Typography.Text code>{paths.log_file}</Typography.Text>
                </Typography.Text>
              </Flex>
            </Card>
          )}
        </Flex>
      </AntApp>
    </ConfigProvider>
  );
}

export default AirCard;
