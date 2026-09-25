import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  App as AntApp,
  Card,
  ConfigProvider,
  Descriptions,
  Empty,
  Flex,
  List,
  Tag,
  Typography,
} from "antd";

type AppPaths = { backups: string; artwork_cache: string; log_file: string };

type DeviceInfo = {
  udid: string;
  name: string;
  product: string;
  version: string;
  build: string;
  language: string;
  locale: string;
  connection: string;
  bold_text: boolean | null;
};

/**
 * What the app knows right now: which iPhones it can reach, and which cards on
 * this Mac still have their original artwork saved.
 *
 * The saved originals were written by the previous implementation, in the same
 * layout, so this doubles as proof that the Rust core reads that data unchanged.
 * Flashing and scanning come next.
 */
function AirCard() {
  const [paths, setPaths] = useState<AppPaths | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [deviceProblem, setDeviceProblem] = useState<string | null>(null);
  const [originals, setOriginals] = useState<Record<string, string[]>>({});
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      try {
        try {
          setDevices(await invoke<DeviceInfo[]>("list_devices"));
        } catch (error) {
          setDeviceProblem(String(error));
        }

        const found = await invoke<string[]>("devices_with_saved_originals");
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
            Replacement artwork for Apple Wallet cards.
          </Typography.Text>

          {problem && <Alert type="error" showIcon message={problem} />}

          {deviceProblem ? (
            <Alert type="warning" showIcon message={deviceProblem} />
          ) : devices.length === 0 ? (
            <Alert
              type="info"
              showIcon
              message="No iPhone connected"
              description="Connect one over USB and unlock it, then reopen this window."
            />
          ) : (
            devices.map((device) => (
              <Card
                key={device.udid}
                size="small"
                title={
                  <Flex gap={8} align="center">
                    <span>{device.name || "iPhone"}</span>
                    <Tag color={device.connection === "usb" ? "green" : "blue"}>
                      {device.connection}
                    </Tag>
                  </Flex>
                }
              >
                <Descriptions size="small" column={2}>
                  <Descriptions.Item label="Model">{device.product || "—"}</Descriptions.Item>
                  <Descriptions.Item label="iOS">
                    {device.version ? `${device.version} (${device.build})` : "—"}
                  </Descriptions.Item>
                  <Descriptions.Item label="Language">{device.language || "—"}</Descriptions.Item>
                  <Descriptions.Item label="UDID">
                    <Typography.Text code copyable>
                      {device.udid}
                    </Typography.Text>
                  </Descriptions.Item>
                </Descriptions>
              </Card>
            ))
          )}

          <Typography.Title level={5} style={{ marginBottom: 0 }}>
            Saved originals on this Mac
          </Typography.Title>
          {Object.keys(originals).length === 0 ? (
            <Empty description="Nothing saved yet" />
          ) : (
            Object.entries(originals).map(([udid, cards]) => (
              <Card
                key={udid}
                size="small"
                title={<Typography.Text code>{udid}</Typography.Text>}
                extra={<Typography.Text type="secondary">{cards.length} cards</Typography.Text>}
              >
                <List
                  size="small"
                  dataSource={cards}
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
