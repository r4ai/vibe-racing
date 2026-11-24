# Assetto Corsa が出すデータと対応

Assetto Corsa（初代、Kunos）が外へ出す全てのデータと、`vr-ac` がそれぞれをどう扱うかの一覧。
ACC（Assetto Corsa Competizione）・AC EVO は構造が違う別のゲームで、対象外。

## 出力の経路

| 経路                   | 場所・方式                                                       | 内容                                                    | 対応                                   |
| ---------------------- | ---------------------------------------------------------------- | ------------------------------------------------------- | -------------------------------------- |
| 共有メモリ `physics`   | `Local\acpmf_physics`                                            | 自車の物理（毎物理ステップ）                            | 全項目（`--game ac`）                  |
| 共有メモリ `graphics`  | `Local\acpmf_graphics`                                           | 自車の周回・順位・旗・セッション                        | 全項目                                 |
| 共有メモリ `static`    | `Local\acpmf_static`                                             | 車・コース・補助設定（起動時固定）                      | 全項目                                 |
| CrewChief 拡張         | `Local\acpmf_crewchief`<br/>（Python アプリ CrewChiefEx が書く） | 全車の位置・周回・タイム・ピット                        | 全項目。あれば他車に使う               |
| UDP リモートテレメトリ | UDP 9996（ハンドシェイク後に配信）                               | 自車の物理（`RTCarInfo`）<br/>全車の周回完了（`RTLap`） | 全項目（`--game ac-udp`）              |
| セッション結果         | `Documents\Assetto Corsa\out\race_out.json`                      | 終わったセッションの全車の周回                          | 付随データの `info.results` に渡す     |
| 走行記録               | `Documents\Assetto Corsa\ctelemetry\`                            | 暗号化された独自形式                                    | 非対応（仕様非公開）                   |
| 自己ベスト             | `Documents\Assetto Corsa\personalbest.ini`                       | 車とコースごとのベストタイム                            | 非対応（セッション外の記録）           |
| セットアップ           | `Documents\Assetto Corsa\setups\<車>\<コース>\*.ini`             | 保存したセットアップ                                    | 非対応（どれを読み込んだか分からない） |
| リプレイ               | `Documents\Assetto Corsa\replay\*.acreplay`                      | 独自のバイナリ                                          | 非対応（仕様非公開）                   |
| 専用サーバーの UDP     | `acServer` のプラグインポート                                    | 全車の位置・衝突・チャット                              | 非対応（サーバーを運営する側の機能）   |
| Python アプリ API      | ゲーム内の `ac` モジュール                                       | ゲーム内アプリ向け                                      | CrewChief 拡張を経由して使う           |

共有メモリの構造は公式の「AC Shared Memory Documentation」（共有メモリ版 1.7、ゲーム 1.14 以降で同じ）、`#pragma pack(4)`、文字列は UTF-16 の固定長。
各ページは決まった大きさで作られ、後の版で足された項目は末尾にある。足される前の版では 0 のまま。

## 正規化の方針

- 全てのページの全項目を、ゲームの名前のまま `Extras.channels` に出す（`physics.gas`、`graphics.iLastTime` のように）
- 解析に使う値は `Frame` の SI 単位に直す。下の表の「Frame」列
- AC に無い値は `None`。0 を未取得の意味で使う項目（タイム・温度・圧力）は `None` に直す
- AC にはセッションの経過時間・番号が無い。アダプタが数える（下の「セッションの扱い」）
- 座標系は左手系（x 右・y 上・z 前）として、左を正に直す

## 共有メモリ `physics`

| 項目                                         | 単位・意味                       | Frame                                                     |
| -------------------------------------------- | -------------------------------- | --------------------------------------------------------- |
| `packetId`                                   | 更新ごとに増える                 | 新しいデータの判定                                        |
| `gas` / `brake`                              | 0〜1                             | `throttle` / `brake`                                      |
| `fuel`                                       | L                                | `fuel_l`                                                  |
| `gear`                                       | 0 後退・1 中立・2 が 1 速        | `gear`（-1 後退・0 中立）                                 |
| `rpms`                                       | rpm                              | `rpm`                                                     |
| `steerAngle`                                 | -1〜1 の入力（舵角の上限は不明） | チャンネルのみ                                            |
| `speedKmh`                                   | km/h                             | `speed_mps`                                               |
| `velocity[3]`                                | m/s、世界座標                    | チャンネルのみ                                            |
| `accG[3]`                                    | G、車の座標（横・上・前）        | `lat_accel_mps2`（左を正）・`long_accel_mps2`             |
| `wheelSlip[4]`                               | 滑りの量                         | チャンネルのみ                                            |
| `wheelLoad[4]`                               | N                                | `Wheel.load_n`                                            |
| `wheelsPressure[4]`                          | psi                              | `Tyre.hot_kpa`・`Wheel.pressure_kpa`                      |
| `wheelAngularSpeed[4]`                       | rad/s                            | `Wheel.speed_mps`（`tyreRadius` を掛ける）                |
| `tyreWear[4]`                                | 新品 100 の残りのグリップ        | `Tyre.grip`（÷100）。溝ではないので `tread` は空          |
| `tyreDirtyLevel[4]`                          | 汚れ                             | チャンネルのみ                                            |
| `tyreCoreTemperature[4]`                     | ℃                                | `Tyre.core_c`                                             |
| `camberRAD[4]`                               | rad                              | `Wheel.camber_rad`                                        |
| `suspensionTravel[4]`                        | m                                | `Wheel.travel_m`、差分から `Wheel.shock_mps`              |
| `drs`                                        | 0/1                              | `drs_active`（`hasDRS` の車）                             |
| `tc` / `abs`                                 | 滑り率の上限（0 は切）           | 車内設定 `traction_control` / `abs`                       |
| `heading` / `pitch` / `roll` / `cgHeight`    | rad・m                           | チャンネルのみ                                            |
| `carDamage[5]`                               | 前・後・左・右（5 つ目は未使用） | `Damage`（400 で全損として 0〜1）、5% 以上で警告 `damage` |
| `numberOfTyresOut`                           | コース外の輪の数                 | 3 輪以上で `surface: off_track`                           |
| `pitLimiterOn`                               | 0/1                              | 警告 `pit_limiter`                                        |
| `kersCharge` / `kersInput` / `kersCurrentKJ` | 0〜1・0〜1・kJ                   | `battery_pct`（KERS・ERS の車）、他はチャンネル           |
| `autoShifterOn`                              | 0/1                              | チャンネルのみ                                            |
| `rideHeight[2]`                              | m、前・後                        | `Wheel.ride_height_m`（前を左右前輪、後を左右後輪）       |
| `turboBoost`                                 | bar                              | チャンネルのみ                                            |
| `ballast`                                    | kg                               | `weight_penalty_kg`                                       |
| `airDensity`                                 | kg/m³                            | チャンネルのみ                                            |
| `airTemp` / `roadTemp`                       | ℃                                | `Weather.air_temp_c` / `track_temp_c`                     |
| `localAngularVel[3]`                         | rad/s、車の座標                  | `yaw_rate_rps`（左を正）                                  |
| `finalFF`                                    | ハンドルの力                     | チャンネルのみ                                            |
| `performanceMeter`                           | s、ベストとの差                  | `delta_to_best_s`                                         |
| `engineBrake`                                | 段                               | 車内設定 `engine_brake`（段のある車）                     |
| `ersRecoveryLevel` / `ersPowerLevel`         | 段                               | 車内設定 `ers_recovery` / `ers_power`（ERS の車）         |
| `ersHeatCharging` / `ersIsCharging`          | 0/1                              | チャンネルのみ                                            |
| `drsAvailable` / `drsEnabled`                | 0/1                              | `drs_active` は `drsEnabled`、他はチャンネル              |
| `brakeTemp[4]`                               | ℃                                | `Wheel.brake_temp_c`                                      |
| `clutch`                                     | ペダル 0〜1（1 で踏み切り）      | `clutch`（1 − ペダル、1 で繋がる）                        |
| `tyreTempI` / `tyreTempM` / `tyreTempO`      | ℃、内・中・外                    | `Tyre.temp_c`・`Wheel.surface_c`（外・中・内）            |
| `isAIControlled`                             | 0/1                              | チャンネルのみ                                            |
| `tyreContactPoint` / `Normal` / `Heading`    | 世界座標のベクトル               | チャンネルのみ                                            |
| `brakeBias`                                  | 0〜1、前の割合                   | 車内設定 `brake_bias`（%）                                |
| `localVelocity[3]`                           | m/s、車の座標                    | `velocity_x_mps`（前）・`velocity_y_mps`（左）            |

## 共有メモリ `graphics`

| 項目                                              | 単位・意味                           | Frame                                                 |
| ------------------------------------------------- | ------------------------------------ | ----------------------------------------------------- |
| `status`                                          | 0 切・1 リプレイ・2 走行・3 一時停止 | 切は `Disconnected`、リプレイと一時停止はフレーム無し |
| `session`                                         | -1〜6                                | `SessionKind`（下表）                                 |
| `currentTime` / `lastTime` / `bestTime` / `split` | 表示用の文字列                       | チャンネルのみ                                        |
| `completedLaps`                                   | 周                                   | `laps_completed`、`lap` は +1                         |
| `position`                                        | 1 から                               | `position`（CrewChief 拡張が無いとき）                |
| `iCurrentTime` / `iLastTime` / `iBestTime`        | ms、0 や 2³¹−1 は無し                | `current_lap_s` / `last_lap_s` / `best_lap_s`         |
| `sessionTimeLeft`                                 | ms、負は無制限                       | `time_remaining_s`                                    |
| `distanceTraveled`                                | m                                    | チャンネルのみ                                        |
| `isInPit`                                         | 0/1、ピットに停止                    | `surface: in_pit_stall`・`pit.in_stall`               |
| `currentSectorIndex`                              | 0 から                               | セクターの境目の位置を学習して `TrackInfo.sectors`    |
| `lastSectorTime`                                  | ms                                   | チャンネルのみ                                        |
| `numberOfLaps`                                    | 周                                   | `laps_total`（時間制でないとき）                      |
| `tyreCompound`                                    | 名前                                 | `tyre_compound`                                       |
| `replayTimeMultiplier`                            | 倍率                                 | チャンネルのみ                                        |
| `normalizedCarPosition`                           | 0〜1                                 | `lap_dist_pct`                                        |
| `carCoordinates[3]`                               | m、世界座標                          | チャンネルのみ                                        |
| `penaltyTime`                                     | s                                    | `penalty_s`                                           |
| `flag`                                            | 0〜6                                 | `flags`（下表）                                       |
| `idealLineOn`                                     | 0/1                                  | チャンネルのみ                                        |
| `isInPitLane`                                     | 0/1                                  | `on_pit_road`                                         |
| `surfaceGrip`                                     | 0〜1                                 | `Weather.track_grip`                                  |
| `mandatoryPitDone`                                | 0/1                                  | `pit.mandatory_done`                                  |
| `windSpeed` / `windDirection`                     | km/h・度                             | `Weather.wind_mps` / `wind_dir_deg`                   |

| `session` | 意味           | `SessionKind` |
| --------- | -------------- | ------------- |
| -1        | 不明           | `other`       |
| 0         | 練習           | `practice`    |
| 1         | 予選           | `qualify`     |
| 2         | 決勝           | `race`        |
| 3         | ホットラップ   | `practice`    |
| 4         | タイムアタック | `practice`    |
| 5         | ドリフト       | `other`       |
| 6         | ドラッグ       | `other`       |

| `flag` | 意味         | `Flag`                 |
| ------ | ------------ | ---------------------- |
| 0      | 無し         | 緑（決勝の走行中のみ） |
| 1      | 青           | `blue`                 |
| 2      | 黄           | `yellow`               |
| 3      | 黒           | `black`                |
| 4      | 白（最終周） | `white`                |
| 5      | チェッカー   | `checkered`            |
| 6      | ペナルティ   | `black`                |

## 共有メモリ `static`

| 項目                                                                                                                                                   | Frame・用途                              |
| ------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------- |
| `smVersion` / `acVersion`                                                                                                                              | `info`                                   |
| `numberOfSessions` / `numCars`                                                                                                                         | `info`                                   |
| `carModel` / `carSkin`                                                                                                                                 | 自車の `car_name` / `info`               |
| `track` / `trackConfiguration` / `trackSPlineLength`                                                                                                   | `TrackInfo.name` / `config` / `length_m` |
| `playerName` / `playerSurname` / `playerNick`                                                                                                          | 自車の `driver`（名と姓）                |
| `sectorCount`                                                                                                                                          | セクターの数（境目の学習）               |
| `maxTorque` / `maxPower` / `maxRpm` / `maxTurboBoost`                                                                                                  | `info`                                   |
| `maxFuel`                                                                                                                                              | `fuel_capacity_l`                        |
| `suspensionMaxTravel[4]` / `tyreRadius[4]`                                                                                                             | `info`、車輪の速さの計算                 |
| `deprecated_1` / `deprecated_2`                                                                                                                        | 使わない                                 |
| `penaltiesEnabled` / `aidFuelRate` / `aidTireRate` / `aidMechanicalDamage` / `aidAllowTyreBlankets` / `aidStability` / `aidAutoClutch` / `aidAutoBlip` | `info`                                   |
| `hasDRS` / `hasERS` / `hasKERS` / `kersMaxJ` / `ersMaxJ`                                                                                               | どの値を出すかの判定、`info`             |
| `engineBrakeSettingsCount` / `ersPowerControllerCount`                                                                                                 | 車内設定を出すかの判定、`info`           |
| `isTimedRace` / `hasExtraLap`                                                                                                                          | 周回数を出すかの判定、`info`             |
| `reversedGridPositions`                                                                                                                                | `info`                                   |
| `PitWindowStart` / `PitWindowEnd`                                                                                                                      | `pit.window`（時間制は分、周回制は周）   |

## CrewChief 拡張 `acpmf_crewchief`

CrewChief の Python アプリ（CrewChiefEx、64 台版と 128 台版）が、ゲーム内の `ac` モジュールから全車を書く。
構造は `numVehicles`・`focusVehicle`・`serverName[512]`・`vehicle[]`。台数の上限が版で違うので、`numVehicles` 台だけ読む。
車 0 が自車。

| 項目                                                        | Frame                                                                   |
| ----------------------------------------------------------- | ----------------------------------------------------------------------- |
| `carId`                                                     | `idx`                                                                   |
| `driverName` / `carModel`                                   | `driver` / `car_name`                                                   |
| `speedMS`                                                   | チャンネルのみ                                                          |
| `bestLapMS` / `lastLapTimeMS` / `currentLapTimeMS`          | `best_lap_s` / `last_lap_s` / チャンネルのみ                            |
| `lapCount`                                                  | `laps_completed`                                                        |
| `currentLapInvalid`                                         | チャンネルのみ                                                          |
| `worldPosition`                                             | チャンネルのみ                                                          |
| `isCarInPitline` / `isCarInPit`                             | `on_pit_road` / `surface: in_pit_stall`                                 |
| `carLeaderboardPosition` / `carRealTimeLeaderboardPosition` | `position`（決勝は走行順、他は順位表）                                  |
| `spLineLength`                                              | `lap_dist_pct`                                                          |
| `isConnected`                                               | 接続していない車は除く                                                  |
| `suspensionDamage[4]` / `engineLifeLeft`                    | 車 0 の値を `Damage.suspension`（0〜1）/ `Damage.engine`（1000 が新品） |
| `tyreInflation[4]`                                          | チャンネルのみ                                                          |

## UDP リモートテレメトリ

ゲームの PC の 9996 番へ `handshaker`（`operationId` 0）を送り、応答の後に 1（`RTCarInfo`）と 2（`RTLap`）を登録する。
共有メモリを読めない別の PC や OS から使う。セッションの種類と残り時間は届かない。

| 構造               | 項目                                                                                                                                                    | Frame                                    |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------- |
| 応答（408 バイト） | `carName` / `driverName` / `trackName` / `trackConfig`                                                                                                  | 自車・コース                             |
| 応答               | `identifier`（4242）/ `version`                                                                                                                         | `info`                                   |
| `RTCarInfo`（328） | `speed_Kmh` / `speed_Mph` / `speed_Ms`                                                                                                                  | `speed_mps`                              |
| `RTCarInfo`        | `isAbsEnabled` / `isAbsInAction` / `isTcEnabled` / `isTcInAction`                                                                                       | `abs_active`、他はチャンネル             |
| `RTCarInfo`        | `isInPit` / `isEngineLimiterOn`                                                                                                                         | `on_pit_road` / 警告 `rev_limiter`       |
| `RTCarInfo`        | `accG_vertical` / `accG_horizontal` / `accG_frontal`                                                                                                    | 横・前後の加速度                         |
| `RTCarInfo`        | `lapTime` / `lastLap` / `bestLap` / `lapCount`                                                                                                          | 周回とタイム                             |
| `RTCarInfo`        | `gas` / `brake` / `clutch` / `engineRPM` / `steer` / `gear`                                                                                             | 入力・回転・ギア（`steer` はチャンネル） |
| `RTCarInfo`        | `cgHeight` / `carSlope` / `carCoordinates`                                                                                                              | チャンネルのみ                           |
| `RTCarInfo`        | `wheelAngularSpeed` / `tyreRadius` / `load` / `camberRAD`                                                                                               | `Wheel`                                  |
| `RTCarInfo`        | `slipAngle` / `slipAngle_ContactPatch` / `slipRatio` / `tyreSlip` / `ndSlip` / `Dy` / `Mz` / `tyreDirtyLevel` / `tyreLoadedRadius` / `suspensionHeight` | チャンネルのみ                           |
| `RTCarInfo`        | `carPositionNormalized`                                                                                                                                 | `lap_dist_pct`                           |
| `RTLap`（212）     | `carIdentifierNumber` / `lap` / `driverName` / `carName` / `time`                                                                                       | 他車の周回数・前の周のタイム             |

## セッションの扱い

AC の共有メモリには、セッションの番号も経過時間も無い。アダプタが次のように数える。

- **経過時間**: `status` が走行中で `packetId` が進む間の実時間を足す（1 回に 1 秒まで）
- **番号**: `session`（種類）、車、コースが変わったら増やす
- **やり直し**: 周回数が減る、走行距離が 50 m 以上減る、時間制の残りが 5 秒以上増えたら `uid` を変える
- **セクターの境目**: `currentSectorIndex` が変わった位置を覚え、全部そろったら `TrackInfo.sectors` に出す
- **フェーズ**: チェッカー旗で `checkered`。決勝のスタート前（0 周・周のタイム 0）は `warmup`。他は `racing`

## 実機で確かめていないこと

公式の文書が単位や向きを書いていない項目は、次の解釈で正規化している。チャンネルには元の値が残る。

| 項目                                          | 解釈                             |
| --------------------------------------------- | -------------------------------- |
| `accG` / `localVelocity` / `localAngularVel`  | 左手系（x 右・y 上・z 前）       |
| `clutch`                                      | ペダルの踏み込み（1 で切れる）   |
| `tyreWear`                                    | 新品 100 からの残りのグリップ    |
| `carDamage`                                   | 400 で全損（CrewChief の読み方） |
| `sessionTimeLeft`                             | ms                               |
| `windSpeed`                                   | km/h                             |
| `performanceMeter`                            | ベストとの差（s、負が速い）      |
| CrewChief の `carRealTimeLeaderboardPosition` | 0 から数える走行順               |
