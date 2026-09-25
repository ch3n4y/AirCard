import { useEffect, useState } from "react";
import {
  App as AntApp,
  Alert,
  Button,
  Card,
  Checkbox,
  Empty,
  Flex,
  Space,
  Tag,
  Typography,
} from "antd";
import { ReloadOutlined } from "@ant-design/icons";

import { api, type AppPaths, type Leftover } from "../api";
import { t } from "../strings";
import type { Run } from "../ui";

type Props = {
  udid: string | null;
  paths: AppPaths | null;
  busy: string | null;
  run: Run;
};

/**
 * 维护：中断留下的残留、这台 Mac 上的位置、日志。
 *
 * 这些都不该出现在主流程里，但每一样都要能自己找到——出问题时，人需要一个能看到
 * 「到底留下了什么」的地方。
 */
export function MaintenancePanel({ udid, paths, busy, run }: Props) {
  const { message } = AntApp.useApp();
  const [leftovers, setLeftovers] = useState<Leftover[] | null>(null);
  const [picked, setPicked] = useState<string[]>([]);
  const [log, setLog] = useState("");

  const loadLeftovers = async () => {
    if (!udid) return;
    const found = await api.leftovers(udid);
    setLeftovers(found);
    // 默认勾上的是本应用生成的名称：别的来源的名称只列出。
    setPicked(found.filter((item) => item.ours).map((item) => item.name));
  };

  const loadLog = async () => setLog(await api.logTail());

  useEffect(() => {
    setLeftovers(null);
    setPicked([]);
  }, [udid]);

  useEffect(() => {
    void loadLog();
  }, []);

  const locations: Array<{ label: string; path: string | undefined }> = [
    { label: t.backupsPath, path: paths?.backups },
    { label: t.artworkCachePath, path: paths?.artwork_cache },
    { label: t.cardsFilePath, path: paths?.cards_file },
    { label: t.logPath, path: paths?.log_file },
  ];

  return (
    <Flex vertical gap={12}>
      <Card
        size="small"
        title={t.leftovers}
        extra={
          <Space size={4}>
            <Button
              size="small"
              icon={<ReloadOutlined />}
              disabled={!udid || busy !== null}
              onClick={() => run(t.refresh, loadLeftovers)}
            >
              {t.refresh}
            </Button>
            <Button
              size="small"
              danger
              disabled={busy !== null || picked.length === 0}
              onClick={() =>
                run(t.sweepSelected, async () => {
                  if (!udid) return;
                  await api.sweepLeftovers(udid, picked);
                  await loadLeftovers();
                  message.success(t.cleanupDone);
                })
              }
            >
              {t.sweepSelected}
            </Button>
          </Space>
        }
      >
        <Flex vertical gap={8}>
          <Typography.Text type="secondary">{t.leftoversHelp}</Typography.Text>
          {leftovers === null ? (
            <Button
              size="small"
              disabled={!udid || busy !== null}
              onClick={() => run(t.refresh, loadLeftovers)}
            >
              {t.refresh}
            </Button>
          ) : leftovers.length === 0 ? (
            <Empty description={t.noLeftovers} image={Empty.PRESENTED_IMAGE_SIMPLE} />
          ) : (
            <Flex vertical gap={6}>
              <Typography.Text type="secondary">{t.ourNamesOnly}</Typography.Text>
              {leftovers.map((item) => (
                <Flex key={item.name} gap={8} align="center">
                  <Checkbox
                    checked={picked.includes(item.name)}
                    disabled={busy !== null || !item.ours}
                    onChange={(event) =>
                      setPicked(
                        event.target.checked
                          ? [...picked, item.name]
                          : picked.filter((name) => name !== item.name),
                      )
                    }
                  />
                  <Typography.Text className="hash">{item.name}</Typography.Text>
                  <Tag color={item.ours ? "orange" : "default"}>
                    {item.ours ? t.oursTag : t.foreignTag}
                  </Tag>
                </Flex>
              ))}
            </Flex>
          )}
        </Flex>
      </Card>

      <Card size="small" title={t.pathsOnThisMac}>
        <Flex vertical gap={8}>
          {locations.map(({ label, path }) => (
            <Flex key={label} gap={8} align="center" wrap>
              <Typography.Text style={{ width: 90 }}>{label}</Typography.Text>
              <Typography.Text type="secondary" className="hash" style={{ flex: 1 }}>
                {path ?? "—"}
              </Typography.Text>
              <Button
                size="small"
                disabled={!path}
                onClick={() => path && void api.openPath(path)}
              >
                {t.open}
              </Button>
            </Flex>
          ))}
        </Flex>
      </Card>

      <Card
        size="small"
        title={t.log}
        extra={
          <Button size="small" icon={<ReloadOutlined />} onClick={() => void loadLog()}>
            {t.refresh}
          </Button>
        }
      >
        {log.trim().length === 0 ? (
          <Alert type="info" showIcon message={t.logEmpty} />
        ) : (
          <pre className="log-block">{log}</pre>
        )}
      </Card>
    </Flex>
  );
}
