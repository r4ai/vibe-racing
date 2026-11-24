# Automobilista 2 のテレメトリ

Automobilista 2（AMS2）が外へ出すデータと、`vr-ams2` がそれぞれをどう扱うかの一覧。
正本はゲームに同梱の `Support/SharedMemory/AMS2_SharedMemoryExampleApp/SharedMemory.h`（`SHARED_MEMORY_VERSION = 14`）。
配置は [`layout.rs`](../src/layout.rs) が C の並びから計算し、テストで既知のオフセットと照合する。

## 出力経路

| 経路                       | 中身                                                              | 扱い                                                             |
| -------------------------- | ----------------------------------------------------------------- | ---------------------------------------------------------------- |
| 共有メモリ `$pcars2$`      | Project CARS 2 形式＋ AMS2 の追加（v10〜v14）。全項目が毎フレーム | 読む。ゲームの設定 System → Shared Memory を「Project CARS 2」に |
| 共有メモリ（PCARS 1 形式） | 旧形式。項目が少なく AMS2 の追加なし                              | 読まない。版が 9 未満なら設定の変更を促すエラー                  |
| UDP（PCARS 2 形式）        | 共有メモリと同じ値を量子化してパケットに分けた放送                | 読まない。下記                                                   |
| 記録ファイル               | なし（リプレイは独自形式で、外から読めない）                      | なし。`vr serve --record` が正規化した `Frame` を残す            |

UDP は共有メモリの部分集合で、値は丸められ（温度は整数、ブレーキバイアスは 1 バイト）、パケットは落ちる。
UDP にだけある値は、各車の今の周のタイムと最後のセクタータイム（`sTimingsData`・`sTimeStatsData`）くらいで、これは区間の通過時刻から同じものを求められる。
同じ PC で動く `vr` には共有メモリで足りるため、UDP は実装しない。

## 読み方

- 版: `mVersion` を見て、その版に無い項目（v10 以降の追加）は欠損として扱う。v14 より新しい版は、末尾に足された項目を無視して読む
- 整合性: ゲームは書き込みの前後で `mSequenceNumber` を 1 つずつ進める。奇数の間は書き込み中。複製の前後で同じ偶数のときだけ使う
- 頻度: ゲームは描画フレームごとに書く。`Frame` は最大 60 Hz に間引く（同じ値なら出さない）
- 自車: `mViewedParticipantIndex` はカメラで見ている車。運転中（`GAME_INGAME_PLAYING`）の値を自車とし、他車を見ている間も保つ（CrewChief と同じ）
- 文字列: NUL 終端の UTF-8（不正な列は置換）

## ゲームが出さず、アダプタが補う値

| 値                           | 求め方                                                                                                                          |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| セッション時刻 `time_s`      | 時間制なら `mSessionDuration − mEventTimeRemaining`。周回制はゲームが進んでいる間の実時間の和                                   |
| セッション番号・`uid`        | 種類（`mSessionState`）・コース・車の変化、`GAME_INGAME_RESTARTING` で 1 つ進める                                               |
| 先頭との差 `gap_to_leader_s` | レース中、各車がライン（周回数の増加）を越えた時刻を記録し、同じ周の先頭の通過時刻と比べる                                      |
| クラス順位・クラス ID        | クラス名ごとの総合順位の並び。ID はクラス名のハッシュ                                                                           |
| セクターの境目               | 各車の `mCurrentSector` が進んだ地点（周の割合）の中央値。全セクター分そろってから出す                                          |
| 燃料の消費 `fuel_use_lph`    | 燃料の量の減り方（2 秒以上の窓）。給油・リセットの増加は捨てる                                                                  |
| 車輪の速さ                   | `mTyreRPS`（実際は rad/s、前進で負）× 半径。半径はアクセル・ブレーキ・横 G の小さい 15 m/s 以上の走行から推定（単位に依らない） |
| 横の車 `alongside`           | 全車の位置と自車の向きから、車 1 台分の前後に重なる 1〜8 m 横の車                                                               |
| ゼッケン `number`            | AMS2 はゼッケンを出さない。参加者の枠番号＋1                                                                                    |

## 項目ごとの扱い

「channel」は `Extras.channels` にゲームの名前のまま出すだけで、解析には使わない。
全項目が channel にも出る（`mParticipantInfo` は `mParticipantInfo.mName` のように 64 要素の配列）。

### ゲームとセッション

| 項目                                     | 扱い                                                                                                                      |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `mVersion`・`mBuildVersionNumber`        | 版の判定、`Extras.info`                                                                                                   |
| `mGameState`                             | 終了・メニュー・リプレイは `Disconnected`。一時停止は時刻を止める。再開始はセッション番号を進める                         |
| `mSessionState`                          | `kind`。練習・テスト・タイムアタックは practice、予選、フォーメーションラップとレースは race                              |
| `mRaceState`                             | `phase`。未スタートは get_in_car、走行中は racing、完走・失格・リタイア・DNF は checkered。フォーメーションは parade_laps |
| `mViewedParticipantIndex`                | `player_car_idx`（上記）                                                                                                  |
| `mNumParticipants`                       | 読む車の数                                                                                                                |
| `mLapsInEvent`                           | `laps_total`（0 は時間制）                                                                                                |
| `mEventTimeRemaining`                    | `time_remaining_s`（秒。ヘッダはミリ秒と書くが実際は秒、終了後は −1）                                                     |
| `mSessionDuration`                       | `time_total_s`（分 → 秒）                                                                                                 |
| `mSessionAdditionalLaps`                 | `Extras.info`。時間の後に走る周回                                                                                         |
| `mSessionIsPrivate`                      | `Extras.info`                                                                                                             |
| `mTrackLocation`・`mTrackVariation`      | `TrackInfo.name`・`config`（英語の短い名前。言語によらない）                                                              |
| `mTranslatedTrackLocation`・`…Variation` | `Extras.info`                                                                                                             |
| `mTrackLength`                           | `TrackInfo.length_m`、周の割合の分母                                                                                      |
| `mNumSectors`                            | セクターの境目の数                                                                                                        |

### 参加者（64 枠）

| 項目                                | 扱い                                                                                 |
| ----------------------------------- | ------------------------------------------------------------------------------------ |
| `mIsActive`                         | 偽の枠は出さない                                                                     |
| `mName`                             | `driver`                                                                             |
| `mWorldPosition`                    | `alongside`、channel                                                                 |
| `mCurrentLapDistance`               | `lap_dist_pct`（÷ コース長。ピットでライン後しばらく 0 のまま）                      |
| `mRacePosition`                     | `position`（0 は未定）                                                               |
| `mLapsCompleted`・`mCurrentLap`     | `laps_completed`・`lap`                                                              |
| `mCurrentSector`                    | セクターの境目の推定                                                                 |
| `mCarNames`・`mCarClassNames`       | `car_name`・`class_name`                                                             |
| `mLastLapTimes`・`mFastestLapTimes` | `last_lap_s`・`best_lap_s`（−1 は欠損）                                              |
| `mCurrentSector1〜3Times`           | `CarExtra.sector_s`                                                                  |
| `mFastestSector1〜3Times`           | `CarExtra.best_sector_s`                                                             |
| `mLapsInvalidated`                  | `CarExtra.lap_invalidated`                                                           |
| `mRaceStates`                       | 完走は車の旗 checkered、失格は disqualify、リタイア・DNF は `CarExtra.retired`       |
| `mPitModes`                         | `on_pit_road`・`surface`（停止中とガレージは in_pit_stall、入口は approaching_pits） |
| `mPitSchedules`                     | ドライブスルー・ストップ＆ゴーは車の旗 black                                         |
| `mHighestFlagColours`               | 車の旗（下表）                                                                       |
| `mHighestFlagReasons`               | channel                                                                              |
| `mOrientations`                     | channel                                                                              |
| `mSpeeds`                           | `CarExtra.speed_mps`                                                                 |
| `mNationalities`                    | `Extras.info`                                                                        |

### 旗

| 値                                 | 正規化                                        |
| ---------------------------------- | --------------------------------------------- |
| `FLAG_COLOUR_GREEN`                | green                                         |
| `FLAG_COLOUR_BLUE`                 | blue                                          |
| `FLAG_COLOUR_WHITE_SLOW_CAR`       | slow_car                                      |
| `FLAG_COLOUR_WHITE_FINAL_LAP`      | white                                         |
| `FLAG_COLOUR_RED`                  | red                                           |
| `FLAG_COLOUR_YELLOW`               | yellow_waving（その車の近く）                 |
| `FLAG_COLOUR_DOUBLE_YELLOW`        | yellow_waving と yellow                       |
| `FLAG_COLOUR_BLACK_AND_WHITE`      | black_white（走り方への警告）                 |
| `FLAG_COLOUR_BLACK_ORANGE_CIRCLE`  | repair                                        |
| `FLAG_COLOUR_BLACK`                | disqualify（AMS2 では失格）                   |
| `FLAG_COLOUR_CHEQUERED`            | checkered                                     |
| `mYellowFlagState` 保留〜再開前    | caution。最終周と再開前は one_lap_to_green も |
| `mYellowFlagState` `YFS_RACE_HALT` | caution と red                                |
| `mPitSchedule` ペナルティ          | black                                         |

`mYellowFlagState` はピットの開閉にも使う（保留・閉鎖中は `pit.open=false`、先頭周のみ開放は自車が先頭周のときだけ開）。

### 自車の状態

| 項目                                                                | 扱い                                                                                                                                                                  |
| ------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `mUnfilteredThrottle`・`Brake`・`Steering`・`Clutch`                | channel                                                                                                                                                               |
| `mThrottle`・`mBrake`                                               | `throttle`・`brake`                                                                                                                                                   |
| `mClutch`                                                           | `clutch`（ペダル量なので 1 − 値）                                                                                                                                     |
| `mSteering`                                                         | channel。−1〜1 の割合で、舵角（rad）に直せないため `steering_rad` は欠損                                                                                              |
| `mCarName`・`mCarClassName`                                         | 自車の `car_name`・`class_name`                                                                                                                                       |
| `mLapInvalidated`                                                   | 自車の `CarExtra.lap_invalidated`                                                                                                                                     |
| `mBestLapTime`・`mLastLapTime`                                      | 自車の `best_lap_s`・`last_lap_s` の補い                                                                                                                              |
| `mCurrentTime`                                                      | `current_lap_s`                                                                                                                                                       |
| `mSplitTimeAhead`・`Behind`・`mSplitTime`                           | channel                                                                                                                                                               |
| `mPersonalFastestLapTime`・`mWorldFastestLapTime`、各セクターの記録 | channel（セッション外の記録）                                                                                                                                         |
| `mFastestSector1〜3Time`（セッション最速）                          | channel                                                                                                                                                               |
| `mPitMode`                                                          | `pit.in_stall`・`pit.service_active`（停止中）                                                                                                                        |
| `mPitSchedule`                                                      | `pit.planned`（自分・エンジニア・損傷・義務・ドライブスルー・ストップ＆ゴー・枠の空き待ち）                                                                           |
| `mEnforcedPitStopLap`                                               | `pit.window`（その周だけの窓）                                                                                                                                        |
| `mCarFlags`                                                         | ABS → `abs_active`、TCS → `tc_active`、リミッター → `pit_limiter`、エンジン警告 → `engine`、エンジン停止（走行中）→ `stalled`。ライト・サイドブレーキ・SCS は channel |
| `mOilTempCelsius`・`mWaterTempCelsius`                              | `oil_temp_c`・`water_temp_c`                                                                                                                                          |
| `mOilPressureKPa`・`mFuelPressureKPa`                               | `oil_press_bar`・`fuel_press_bar`                                                                                                                                     |
| `mWaterPressureKPa`                                                 | channel                                                                                                                                                               |
| `mFuelLevel`・`mFuelCapacity`                                       | `fuel_l`（割合 × 容量）・`fuel_capacity_l`                                                                                                                            |
| `mSpeed`・`mRpm`・`mGear`                                           | `speed_mps`・`rpm`・`gear`                                                                                                                                            |
| `mMaxRPM`・`mNumGears`・`mOdometerKM`                               | channel                                                                                                                                                               |
| `mAntiLockActive`                                                   | `abs_active`                                                                                                                                                          |
| `mLastOpponentCollisionIndex`・`Magnitude`                          | channel                                                                                                                                                               |
| `mBoostActive`・`mBoostAmount`                                      | `p2p_active`・`p2p_count`（残り %、車に無ければ欠損）                                                                                                                 |
| `mTurboBoostPressure`                                               | channel                                                                                                                                                               |
| `mBrakeBias`                                                        | 車内設定 `brake_bias`（値 × 100。前後どちらの割合かは SDK に記載なし）                                                                                                |
| `mAntiLockSetting`・`mTractionControlSetting`                       | 車内設定 `abs`・`traction_control`（−1 は無し）                                                                                                                       |
| `mErsDeploymentMode`                                                | 車内設定 `ers_mode`（0 は無し）                                                                                                                                       |
| `mErsAutoModeEnabled`・`mLaunchStage`                               | channel                                                                                                                                                               |
| `mDrsState`                                                         | 装備時のみ `drs_active`・`drs_available`                                                                                                                              |
| `mClutchTemp`・`mClutchWear`・`mClutchSlipping`                     | channel                                                                                                                                                               |
| `mClutchOverheated`                                                 | 警告 `clutch`                                                                                                                                                         |
| `mJoyPad0`・`mDPad`                                                 | channel                                                                                                                                                               |

### 動き

座標は CrewChief のスポッターと同じ解釈（車の前が −Z、左が +X、Y が上）。

| 項目                                                     | 扱い                                            |
| -------------------------------------------------------- | ----------------------------------------------- |
| `mLocalVelocity`                                         | `velocity_x_mps`（−Z）・`velocity_y_mps`（+X）  |
| `mLocalAcceleration`                                     | `long_accel_mps2`（−Z）・`lat_accel_mps2`（+X） |
| `mAngularVelocity`                                       | `yaw_rate_rps`（−Y）                            |
| `mOrientation`                                           | `alongside` の向き、channel                     |
| `mWorldVelocity`・`mWorldAcceleration`・`mExtentsCentre` | channel                                         |

### タイヤ・車輪

AMS2 のタイヤの値は走行中も変わる（`Tyres.live`）。

| 項目                                                                       | 扱い                                                         |
| -------------------------------------------------------------------------- | ------------------------------------------------------------ |
| `mTyreTempLeft`・`Center`・`Right`                                         | `temp_c`（外・中・内。右のタイヤは左右を入れ替え）           |
| `mTyreTemp`                                                                | 上が未設定（0）のときの `temp_c` の中                        |
| `mTyreTreadTemp`・`mTyreLayerTemp`・`mTyreRimTemp`・`mTyreInternalAirTemp` | channel（ケルビン）                                          |
| `mTyreCarcassTemp`                                                         | `Tyre.core_c`                                                |
| `mTyreWear`                                                                | `tread` の中（1 − 摩耗）                                     |
| `mAirPressure`                                                             | `hot_kpa`・車輪の `pressure_kpa`（70 未満は PSI として換算） |
| `mBrakeTempCelsius`                                                        | 車輪の `brake_temp_c`                                        |
| `mTyreCompound`                                                            | `tyre_compound`（前後で違えば「前/後」）                     |
| `mTyreFlags`                                                               | 外れた・空気の抜けたタイヤは警告 `flat_tyre`、channel        |
| `mTerrain`                                                                 | 4 輪とも舗装外なら自車の `surface=off_track`、channel        |
| `mTyreRPS`                                                                 | 車輪の `speed_mps`（上記）、channel                          |
| `mRideHeight`                                                              | 車輪の `ride_height_m`（1 を超える値は cm として換算）       |
| `mSuspensionVelocity`                                                      | 車輪の `shock_mps`                                           |
| `mSuspensionTravel`                                                        | 車輪の `travel_m`                                            |
| `mWheelLocalPositionY`・`mTyreY`・`mTyreHeightAboveGround`                 | channel                                                      |
| `mTyreSlipSpeed`・`mTyreGrip`・`mTyreLateralStiffness`                     | 廃止された項目。channel にも出さない                         |

### 損傷

| 項目                                | 扱い                                                                     |
| ----------------------------------- | ------------------------------------------------------------------------ |
| `mAeroDamage`・`mEngineDamage`      | `damage.aero`・`damage.engine`（0〜1）。どこかが 0.2 以上で警告 `damage` |
| `mBrakeDamage`・`mSuspensionDamage` | `damage.brakes`・`damage.suspension`（LF・RF・LR・RR）                   |
| `mCrashState`                       | コース外（`CRASH_DAMAGE_OFFTRACK`）は `surface=off_track`、channel       |

### 天候

| 項目                                       | 扱い                                        |
| ------------------------------------------ | ------------------------------------------- |
| `mAmbientTemperature`・`mTrackTemperature` | `air_temp_c`・`track_temp_c`                |
| `mRainDensity`                             | `precipitation`                             |
| `mSnowDensity`                             | channel                                     |
| `mWindSpeed`                               | `wind_mps`                                  |
| `mWindDirectionX`・`Y`                     | channel（ワールド座標で、北との関係が不明） |
| `mCloudBrightness`                         | channel                                     |

## 使えない解析

| 解析                             | 理由                                               |
| -------------------------------- | -------------------------------------------------- |
| 挙動の傾向（アンダー・オーバー） | 舵角が無い（`mSteering` は割合）                   |
| インシデント                     | ゲームが数えない（`Capabilities.incidents=false`） |
| ガレージのセットアップ           | ゲームが出さない（`Extras.setup` なし）            |
| 事前のピット要求・燃料の指定     | ゲームが出さない                                   |
