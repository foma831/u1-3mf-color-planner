# Техническое задание: нативная генерация проектных 3MF для Snapmaker U1 и Bambu Lab A1 mini

Версия документа: 1.6  
Дата: 3 августа 2026 года  
Статус: Этапы A–D реализованы и GUI-qualified для точных целевых установок  
Связанный документ: `TECHNICAL_SPECIFICATION_RU.md`  
Язык UI, кода, идентификаторов, логов и машинно-читаемых файлов: английский  
Язык данного ТЗ и обсуждения: русский

---

## 0. Текущий статус реализации

| Этап | Область | Статус на 3 августа 2026 года |
|---|---|---|
| A | Generic bounded OPC/3MF writer и structural validator | Реализован |
| B | Snapmaker U1 0.4 Direct Spools | GUI-qualified; conversion доступен при совпадении exact executable/profile evidence и валидном плане |
| C | Bambu Lab A1 mini single-spool | GUI-qualified по отдельным PLA/PETG round trips в exact Bambu Studio 02.02.00.85 |
| D | Snapmaker U1 CMY+X Full Spectrum | GUI-qualified по шестиплатному mixed + solid T4 round trip в exact Snapmaker Orca 2.3.5; физическая точность цвета остаётся per-user evidence |

Этап B завершил GUI qualification 3 августа 2026 года. Реализованный writer
создаёт структурно проверенные U1 Direct projects, а desktop разрешает обычные
`prepare_conversion` и `convert_project` только после совпадения exact version,
executable SHA-256, effective installed profile pack, profile closure,
auxiliary runtime tables и hash-bound qualification документов. Проверенный
GUI-цикл в Snapmaker Orca 2.3.5:

```text
open → slice → save → close → reopen → re-slice → re-save
```

Нельзя разблокировать conversion простым изменением одного boolean. Release
record и отдельный typed qualification report привязаны к exact
application/executable/profile baseline, исходному Sailfin fixture,
candidate-файлу и двум GUI-saved файлам. Record хранит SHA-256 точных bytes
report; несовпадение документов, неизвестное поле или неполный typed check
закрывают gate. Оба документа имеют `qualified` status.

Завершённые qualification-пункты Этапа B:

1. выполнен фактический GUI round trip и установлена exact evidence-запись;
2. доказана GUI compatibility derived profile identity groups и `inherits`
   chains, включая стабильность после двух re-save;
3. candidate, первый GUI save и повторный GUI save прошли version-scoped
   structural/semantic validator с одинаковым semantic SHA-256.

Post-qualification hardening реализован на уровне mixed desktop orchestration:
одноразовый preflight contract, cooperative cancel, recovery registry и startup
cleanup abandoned staging, exact-partition coverage, no-clobber publication и
проверка полного дерева bundle. Gate C/D теперь содержит реальную hash-bound
GUI evidence; успешный structural test по-прежнему не подменяет GUI-проверку,
а Full Spectrum writer qualification не подменяет per-user physical color
calibration или явное fingerprint-bound подтверждение аппроксимации.

Два прежних вопроса уже имеют явную реализационную policy и больше не являются
blockers:

- Stage B использует только exact hash-pinned system profiles и намеренно пишет
  zero process/filament/machine `settings_N.config` entries;
- negative Z сохраняется без lift в соответствии с Orca sinking/clipping
  semantics; полностью находящийся ниже bed объект отклоняется;
- target process globals фиксированы как Textured PEI Plate, by-layer,
  non-spiral и traditional timelapse; эквивалентные source per-plate overrides
  нормализуются, а конфликтующие отклоняются;
- prime tower рассчитывается от lower-left body anchor с консервативным
  printed-height-aware bed/collision envelope;
- source Orca `identify_id` каждого выбранного instance сохраняется.

---

## 1. Решение, принятое по результатам исследования

Приложение должно самостоятельно формировать новые **неслайсенные Project 3MF** для целевого принтера. Оно не должно:

- подменять только имя принтера в исходном Bambu-проекте;
- использовать Snapmaker Orca или Bambu Studio CLI как основной writer;
- автоматизировать GUI слайсера для каждой пользовательской конвертации;
- копировать готовый exporter из Snapmaker Orca, OrcaSlicer или Bambu Studio;
- переносить старый G-code, оценки печати, slice cache и previews как актуальные данные.

Выбранная архитектура:

1. clean-room writer на Rust записывает ZIP/OPC, 3MF XML и Bambu/Orca metadata;
2. versioned target adapters строят настройки U1 и A1 mini из проверенных целевых профилей;
3. неизменённая геометрия копируется потоково, изменяемые XML-записи переписываются потоково;
4. structural validator проверяет результат до публикации;
5. Snapmaker Orca и Bambu Studio используются как независимые qualification validators: открыть, нарезать, сохранить, закрыть, повторно открыть и снова нарезать.

Такой подход соответствует фактическому пути сохранения Project 3MF в официальном коде Snapmaker Orca (`store_bbs_3mf`), но не требует копирования AGPL-кода.

## 2. Цель опции

В окончательной версии после утверждения плана кнопка `Approve & Convert`
должна создать набор редактируемых файлов, которые пользователь открывает и
нарезает в целевом слайсере. Каждый target adapter разрешается только при exact
qualified installation evidence и валидном canonical плане; qualification
candidate не выдаётся как обычный пользовательский результат:

- один U1 3MF на каждый batch с неизменным физическим loadout T1–T4;
- один A1 mini 3MF на каждую физическую катушку и material/profile group;
- `manifest.json` с точным соответствием source unit → output file → target plate;
- `conversion-plan.json` с утверждённым планом и действиями оператора;
- `conversion-report.html` с детерминированным offline-отчётом;
- `checksums.sha256` для контроля целостности.

Writer должен реализовать уже существующие решения планировщика:

- `CMY+X Full Spectrum`;
- `Direct Spools`, включая явное many-to-one объединение исходных цветов;
- отдельные моно-задания;
- явный выбор `Auto / Snapmaker U1 / Bambu Lab A1 mini`;
- сохранение материала либо отдельное подтверждение его замены;
- группировку U1 по T4 и группировку A1 mini по одной физической катушке.

## 3. Результаты исследования контрольных файлов

### 3.1. Контрольные hashes

| Файл | SHA-256 | Назначение в тестах |
|---|---|---|
| `Sample/Withered_Foxy.3mf` | `f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81` | сложный исходный Bambu P1S проект |
| `Sample/Sailfin Dragon - Articulated Lizard by Raki-Box.3mf` | `1b20d6124353d3bc31c4ea554e482ba6f8ef281dd23e2df6bbc0d561b4f6e0d7` | референс структуры U1 и окрашенного mesh |
| `Sample/Withered_Foxy_A1_mini_No_AMS.3mf` | `78a613193c05f96c77e0db5c7a5a0ce0eb051936752ae2b0def09ae8202cab8c` | forensic-референс A1 mini без AMS |

Исходные файлы считаются неизменяемыми fixtures. Тесты обязаны проверять их SHA-256 до и после работы.

### 3.2. U1 Sailfin

Образец подтверждает следующий контракт:

- editable Project 3MF без G-code;
- профиль `Snapmaker U1 (0.4 nozzle)`;
- четыре сопла по 0,4 мм;
- область 270 × 270 × 270,05 мм;
- одна плата, четыре printable build items;
- 3 428 075 vertices и 6 855 906 triangles;
- 2 566 048 окрашенных triangles;
- окраска хранится в `paint_color`, включая рекурсивные TriangleSelector trees;
- вся геометрия находится inline в одном `3D/3dmodel.model` размером более 512 MiB после распаковки;
- project arrays содержат четыре toolheads, однако имена `filament_settings_N.config` не являются номерами T1–T4;
- часть embedded filament presets не используется и противоречит текущим project arrays;
- официальный exporter Snapmaker Orca 2.3.5 сериализует отдельный embedded
  preset только при `is_project_embedded`.

Следствия:

1. analyzer и writer обязаны поддерживать как monolithic model, так и Production Extension split models;
2. текущий общий лимит 512 MiB на ZIP-entry необходимо заменить type-aware streaming limit;
3. source embedded presets нельзя копировать или сопоставлять toolhead по номеру
   файла. Stage B работает только с exact hash-pinned system profiles и поэтому
   создаёт zero `process_settings_N.config`, `filament_settings_N.config` и
   `machine_settings_N.config`; future custom/embedded mode должен будет строить
   и дедуплицировать presets отдельно;
4. `paint_color` можно менять только через существующий decode → remap → encode codec;
5. образец пригоден для U1 Direct Spools и solid writer;
6. образец не содержит `mixed_filament_definitions` и не закрывает Full Spectrum acceptance.

### 3.3. Withered Foxy source

Исходный Bambu-проект содержит:

- 12 source plates;
- 89 objects и 89 instances;
- 390 parts, из них 388 printable positive parts;
- 7 308 333 vertices и 14 616 548 triangles;
- 10 фактически используемых source slots;
- 74 mono и 15 multicolor objects;
- 89 external `3D/Objects/*.model` и Production Extension relationships;
- смешанные PLA/PETG material requirements;
- отсутствующий G-code.

Он является основным end-to-end acceptance input.

У основных плат `Head`, `Torso`, `Frame AMS`, `Armor`, `Legs` и `Jaw`
минимальная мировая Z ниже нуля (примерно от −0,57 до −19,06 мм). Официальные
`BuildVolume`/`PartPlate` в Snapmaker Orca 2.3.5 поддерживают sinking: часть mesh
ниже Z=0 клиппируется build plane. Stage B сохраняет исходную negative Z без
lift и тем самым не меняет печатаемую форму. Writer отклоняет только объект,
который находится ниже bed полностью (`max Z <= 0`), и продолжает
консервативно проверять XY bounds и maximum Z. Negative Z больше не является
blocker.

### 3.4. Withered Foxy A1 mini No AMS

Образец подтверждает реальную структуру A1 mini Project 3MF:

- Bambu Studio dialect `02.06.00.51`;
- `Bambu Lab A1 mini 0.4 nozzle`;
- область 180 × 180 × 180 мм;
- `has_filament_switcher=0`;
- одна эффективная физическая катушка и один используемый logical slot;
- процесс `0.12mm High Quality @BBL A1M`;
- 9 plates, 89 objects, 89 instances и 390 parts;
- те же суммарные 7 308 333 vertices и 14 616 548 triangles;
- отсутствующий G-code.

Платы распределены так:

| Plate | Имя | Objects |
|---:|---|---:|
| 1 | Head | 2 |
| 2 | Armor | 8 |
| 3 | Frame | 9 |
| 4 | Updated Ball Joints | 17 |
| 5 | Alternate Joints | 11 |
| 6 | Hinge Joints | 12 |
| 7 | Hinge Joints | 12 |
| 8 | Hinge Joints | 11 |
| 9 | Frame | 7 |

Этот файл является полезным forensic reference, но **не production golden fixture**, потому что:

- исходные PETG-части на plates 3–9 молча заменены на PLA;
- масштаб трёх компонентов изменён;
- размещение или ориентация как минимум 27 объектов изменены;
- часть IDs, UUID и external model paths пересоздана;
- в metadata осталась soft-ссылка на отсутствующий thumbnail;
- не зафиксирован цикл save → close → reopen → slice;
- установленный локально Bambu Studio `02.02.00.85` старше dialect образца `02.06.00.51`.

Writer запрещено наследовать перечисленные дефекты. Образец используется только для понимания schema, plate layout и нормализации arrays.

## 4. Границы реализации

### 4.1. Входит

- U1 Direct Spools Project 3MF;
- U1 CMY+X Full Spectrum Project 3MF;
- A1 mini single-spool Project 3MF без AMS Lite;
- one-file-per-loadout/batch export;
- детерминированная раскладка по plate;
- перенос неизменной геометрии и разрешённого print intent;
- target-owned machine/process/filament settings;
- remap object-, part- и facet-level assignments;
- структурная и семантическая проверка output;
- интеграция с `Approve & Convert`;
- открытие output в установленном целевом слайсере по явной кнопке пользователя.

### 4.2. Не входит

- генерация готового G-code;
- отправка задания на принтер;
- управление заменой катушек во время одного файла;
- AMS Lite multi-color output;
- автоматическое масштабирование, разрезание или изменение ориентации детали;
- скрытая замена material family;
- копирование cloud/device IDs и приватных данных;
- обещание физической точности Full Spectrum без калибровки.

## 5. Формат выходного пакета

Рекомендуемая структура каталога:

```text
Withered_Foxy__converted/
  u1-full-spectrum/Withered_Foxy__U1__batch-01__Full-Spectrum.3mf
  u1-direct/Withered_Foxy__U1__batch-02__Direct-Spools.3mf
  a1-mini/Withered_Foxy__A1-mini__plate-01__spool.3mf
  manifest.json
  conversion-plan.json
  conversion-report.html
  checksums.sha256
```

Это единый production-контракт desktop mixed writer. Низкоуровневый Stage B
Direct adapter до нормализации создаёт приватный временный bundle собственного
контракта:

```text
<source>__converted/
  <source>__U1__batch-01__Direct-Spools.3mf
  <source>__U1__batch-02__Direct-Spools.3mf
  manifest.json
  print-plan.json
  checksums.sha256
```

Эти adapter-owned metadata не публикуются и удаляются при переносе 3MF в
production bundle. Точное количество `Direct-Spools.3mf` равно количеству U1
Direct batches в canonical backend plan. В опубликованном bundle отсутствуют
snapshot исходника, временные rewrite archives и любые скрытые служебные файлы.

Текущий `manifest.json` содержит:

- schema v2 и adapter ID;
- source leaf-name без host path, размер и SHA-256;
- source application/version/dialect/support из authoritative analysis;
- converter build identity: package version и compile-time
  `U1_PLANNER_GIT_IDENTITY`, либо детерминированное значение `Unknown`;
- capability evidence exact Snapmaker Orca installation;
- canonical plan fingerprint;
- для каждого artifact: batch, имя, размер, SHA-256, target/source plates,
  source units, физический loadout и понятные setup actions; каждый physical
  slot loadout отдельно хранит `profile`, `settingId` и `filamentId`;
- отображение `sourceUnitId + sourcePlateId → outputFile + batchId + targetPlateId`;
- warnings.

`conversion-plan.json` содержит неизменяемую backend serialization planning
input/result вместе с компактным source/build provenance, exclusions, exact
warning acknowledgement и fingerprint, но не дублирует artifact/evidence из
manifest. `conversion-report.html` детерминированно строится из проверенного
manifest. `checksums.sha256` покрывает каждый 3MF, `manifest.json`,
`conversion-plan.json` и `conversion-report.html`.
Каталог публикуется no-clobber atomic rename; существующий каталог с тем же
именем не перезаписывается.

Restart recovery принимает schema v2. Старый schema v1 распознаётся отдельно,
но отклоняется fail-closed с требованием повторной конвертации, поскольку в нём
нет достаточного provenance для безопасной миграции.

Правила:

- один U1-файл содержит только plates одного физического loadout;
- смена T4 выполняется **между файлами**, поэтому принтер не обязан ставить печать на паузу для смены катушки;
- один A1 mini-файл использует ровно одну физическую катушку, один material profile и один nozzle/process adapter;
- в первой production-версии A1 writer выпускает один 3MF на target plate;
- multi-plate A1 output включается отдельным capability flag после qualification виртуальной plate grid соответствующего Bambu dialect;
- имена файлов очищаются от unsafe path characters и остаются стабильными при повторной конвертации;
- существующий output не перезаписывается без отдельного подтверждения;
- source 3MF никогда не является допустимым output path.

## 6. Архитектура

### 6.1. Распределение ответственности по текущему workspace

| Модуль | Новая ответственность |
|---|---|
| `crates/three-mf` | bounded package reader, OPC builder, model copier/rewriter, structural validator |
| `crates/orca-adapter` | U1 2.3.5 target schema, resolved profiles, Full Spectrum serialization, slicer capability report |
| новый `crates/a1mini-adapter` | versioned Bambu Studio A1 mini target schema и profiles |
| `crates/planner` | окончательная раскладка, target plate membership, batch/job graph |
| `crates/application` | canonical conversion request, orchestration, progress, manifest |
| `crates/converter-cli` | `convert`, `validate-output`, `doctor` и qualification commands |
| `apps/desktop/src-tauri` | защищённые native commands и atomic publication |
| `apps/desktop/src` | preflight, progress, result и actionable errors |

### 6.2. Новые основные типы

```text
ConversionRequest
  source_path
  source_sha256
  plan_fingerprint
  destination_directory
  overwrite_policy
  requested_adapter_ids

ConversionJob
  job_id
  target_printer
  target_adapter_id
  physical_loadout
  source_unit_ids
  target_plates
  color_assignments
  material_assignments
  recipe_definitions

TargetPlateLayout
  plate_id
  name
  instances
  translations
  bounds_evidence

ConversionArtifact
  job_id
  temporary_path
  final_path
  sha256
  validation_report

ConversionManifest
  schema_version
  source_identity
  plan_fingerprint
  adapter_evidence
  artifacts
  source_to_target_map
  user_approvals
  warnings
```

Frontend не передаёт writer произвольный план. Backend принимает только `source_sha256` и `plan_fingerprint`, затем использует последний сохранённый canonical backend plan.

## 7. Versioned target adapters

### 7.1. U1 adapter

Начальный ID:

```text
snapmaker-orca/2.3.5/u1-0.4
```

Adapter обязан зафиксировать:

- application version и executable hash;
- machine preset `Snapmaker U1 (0.4 nozzle)` и `setting_id=SM_U1`;
- четыре nozzle diameters по 0,4 мм;
- полный resolved machine profile и inheritance hashes;
- process profile для Direct и Full Spectrum;
- physical filament profiles;
- target project schema version;
- допустимые Full Spectrum modes и их serialized schema;
- cardinality каждого project setting array;
- правила thumbnails, plates и embedded configs.

Системные профили установленного Snapmaker Orca считаются источником target machine settings. Sailfin задаёт структуру, но его старые embedded machine/filament values не имеют приоритета над утверждённым adapter.

### 7.2. A1 mini adapter

Первый гарантированный adapter выбирается только после qualification одной точной версии:

```text
bambu-studio/<exact-version>/a1-mini-0.4-no-ams
```

Dialect `02.06.00.51` из предоставленного sample и установленный Bambu Studio
`02.02.00.85` считаются разными adapters. Были допустимы два пути:

1. установить и квалифицировать Bambu Studio версии `02.06.00.51`;
2. создать отдельные clean fixtures в установленной `02.02.00.85` и поддержать её отдельным adapter.

Реализован второй путь: отдельные clean PLA/PETG candidates прошли GUI
qualification в `02.02.00.85`. Теперь A1 conversion capability возвращает
`qualified` только при точном совпадении executable, effective profile pack и
hash-bound evidence; sample dialect не используется как target adapter.

### 7.3. Compatibility gate

Экспорт конкретного job разрешён только если:

- найден exact target adapter;
- hashes обязательных profile files совпадают;
- profile inheritance полностью разрешён;
- nozzle и build volume совпадают;
- golden fixtures этого adapter прошли structural и GUI qualification;
- текущий job не использует неподдерживаемый material/process/recipe mode.

Обновление слайсера не переиспользует старый adapter автоматически.

## 8. Правила построения Project 3MF

### WR-001. Безопасное открытие source

Writer открывает source один раз, создаёт ограниченный приватный snapshot и
повторно проверяет byte size и SHA-256 как исходного открытого handle, так и
snapshot после формирования пакета. Canonical path и file identity проверяются
отдельно. Writer использует bounded OPC reader и не распаковывает архив в
каталог.

### WR-002. Новый package graph

Output строится по allowlist, а не через копирование всех ZIP entries. Writer заново формирует:

- `[Content_Types].xml`;
- `_rels/.rels`;
- `3D/3dmodel.model`;
- `3D/_rels/3dmodel.model.rels`, если используются external submodels;
- `Metadata/project_settings.config`;
- `Metadata/model_settings.config`;
- `Metadata/_rels/model_settings.config.rels`, если требует adapter;
- embedded machine/process/filament configs только для adapter mode, который
  явно поддерживает `is_project_embedded`; Stage B U1 Direct их не создаёт;
- adapter-approved auxiliary files.

Каждый internal relationship target обязан существовать. Absolute targets, `..`, duplicates и unreferenced required parts запрещены.

### WR-003. Production Extension и геометрия

Предпочтительный output layout — Production Extension split model:

- root model содержит resources/components, необходимые для build;
- mesh submodels хранятся в `3D/Objects`;
- root relationships перечисляют каждый submodel;
- build находится только в root model;
- objects, components и build items получают корректные уникальные UUID;
- writer хранит детерминированный source → target ID map в manifest.

Неизменённый external mesh entry копируется raw/streaming, если его namespace, IDs и assignments не требуют изменения. Monolithic source может быть потоково разделён на submodels, если это необходимо для bounded memory; семантика компонентов при этом сохраняется.

### WR-004. Инвариантность геометрии

Без отдельного пользовательского действия writer не изменяет:

- vertex coordinates;
- triangle indices;
- scale;
- rotation;
- handedness;
- mesh repair state;
- positive/negative/modifier/support role;
- относительные transforms частей внутри printable unit.

Допустимо менять только target-plate translation, рассчитанную packer. После записи validator повторно вычисляет мировые bounds и geometry fingerprints.

Negative Z не считается изменением geometry и сохраняется точно. Для U1
официальная sinking semantics клиппирует часть mesh ниже build plane Z=0.
Запрещено автоматически добавлять Z-lift. Объект с `max Z <= 0` не имеет
печатаемой части и отклоняется.

### WR-005. Plate membership

Для каждого printable instance writer одновременно обновляет:

- root `<build><item transform>`;
- `Metadata/model_settings.config` plate membership;
- соответствующий source Orca `identify_id` в `model_instance` без
  перенумерации или генерации нового значения;
- target plate name/order;
- target translation;
- при необходимости adapter-specific plate coordinate representation.

Каждый обязательный source unit должен встречаться ровно один раз во всём output bundle. Optional/excluded unit перечисляется в manifest с причиной.

Отсутствующий `identify_id` у выбранного source instance является hard error:
writer не угадывает этот Orca identity. При перепаковке меняются plate ID и
translation, но identity экземпляра остаётся исходной.

### WR-006. Раскладка

Production packer обязан:

- использовать target bed polygon и excluded areas из adapter;
- учитывать brim, skirt, object clearance и prime/wipe tower reserve;
- проверять XY intersections и Z height;
- сохранять исходную ориентацию;
- перемещать unit только translation-операцией;
- не разъединять parts, modifiers и support volumes одного unit;
- выдавать детерминированный результат при одинаковом input;
- повторно проверять bounds после serialization.

Для A1 mini применяется 180 × 180 × 180 мм; для U1 — целевой профиль 270 × 270 × 270 мм. AABB individual fit не заменяет финальную packing validation.

Для multicolor U1 plate `wipe_tower_x/y` трактуется точно как lower-left corner
неповёрнутого tower body, а не как его центр. Высота envelope равна фактической
печатной высоте plate (`max Z`), чтобы учитывать расширение 15° stabilization
cone на всю напечатанную высоту. Консервативный XY envelope включает:

- 30 мм body width и worst-case 45 мм depth;
- 5 мм prime-tower brim;
- полный 8 мм rib extension профиля (`wipe_tower_wall_type=rib`);
- 1 мм tower clearance;
- дополнительный 6 мм object-footprint clearance при collision check.

Envelope обязан целиком помещаться в qualified U1 bed bounds и не пересекать
ни один расширенный printable-instance footprint. Для mono plate tower не
печатается, поэтому используется стабильное schema-compatible положение без
обязательной collision reservation.

### WR-007. Target-owned settings

Следующие группы всегда берутся из target adapter:

- printer model, variant, bed geometry и height;
- kinematics, nozzle count/diameters и toolhead offsets;
- machine limits;
- start/end/layer-change/tool-change G-code;
- plate compatibility;
- extruder, purge, ramming и filament-switcher schema;
- target process defaults;
- target filament temperature, flow и volumetric limits;
- target profile identity, inheritance, `setting_id` и `filament_id`.

Запрещено переносить их из P1S source или другого принтера.

Stage B не наследует source process globals, а явно устанавливает:

- `curr_bed_type=Textured PEI Plate`;
- `print_sequence=by layer` и нулевые first/other layer sequence arrays;
- `spiral_mode=0` и `spiral_mode_smooth=0`;
- `timelapse_type=0` (traditional timelapse).

До writer preparation source `model_settings.config` проверяется на per-plate
process overrides. Значения `bed_type=Textured PEI Plate`, `print_sequence=by
layer`, first/other sequence `0`, sequence count `0` и `spiral_mode=false/0`
и `timelapse_type=0` семантически эквивалентны target globals: они
нормализуются, их количество попадает в warning. Дубликат, отсутствующее value,
конфликтующее значение или непустая вложенная process metadata считаются
небезопасными и дают hard error, поскольку writer не может молча удалить
исходное поведение.

Merged target profile обязан явно содержать конечные неотрицательные
`brim_width` и `brim_object_gap`: не более 5 мм и 1 мм соответственно. Такие же
ограничения действуют для сохраняемых object/part overrides. При размещении
prime tower используется более широкий object envelope 19 мм: writer явно
фиксирует `auto_brim`, который в Snapmaker Orca может вырасти до 18 мм, плюс
допускается 1 мм gap.
Отсутствующее target-значение, malformed, отрицательное или большее значение
даёт hard error.

Остальная object/part metadata переносится fail-closed по явному Stage B
allowlist: identity/transform metadata, проверенные internal-print overrides,
известные типы brim и только target-equivalent выключенные/нулевые значения
support, raft и XY compensation. Painted brim блокируется, потому что Stage B
не переносит его point sidecar; bounded automatic `brim_ears` поддерживается.
Неизвестный per-object process override блокирует конвертацию до отдельной
квалификации его влияния на footprint; он не может молча пройти в файл и
нарушить 19-миллиметровый collision proof.

### WR-008. Transferable print intent

Из source можно переносить только явно разрешённые и проверенные intent values:

- layer height в диапазоне target process;
- wall count;
- top/bottom shell intent;
- infill pattern/density;
- support enable/type после совместимости;
- brim/skirt intent;
- seam and quality hints, если adapter их поддерживает.

Каждый key находится в versioned allowlist. Неизвестный key не переносится молча и фиксируется в report.

### WR-009. System и embedded presets

Официальный Snapmaker Orca 2.3.5 exporter записывает отдельный
`process_settings_N.config`, `filament_settings_N.config` или
`machine_settings_N.config` только для preset с `is_project_embedded`. Имя такого
файла является порядковым номером уникального embedded preset, а не номером
toolhead.

Stage B U1 Direct не поддерживает custom/project-embedded profiles. Adapter
разрешает только exact hash-pinned system machine/process/filament profiles,
полностью разрешает их inheritance из тех же проверенных bytes и намеренно
создаёт **zero** embedded `settings_N.config` entries. Structural validator
считает наличие любого такого entry hard error.

На macOS источником этих bytes является не обязательно bundled seed внутри
`.app`. Snapmaker Orca сначала обновляет и затем загружает effective vendor pack
из `~/Library/Application Support/Snapmaker_Orca/system/Snapmaker`. Capability
обязана зафиксировать source kind, vendor-pack version, SHA-256 `Snapmaker.json`,
19 файлов замыкания inheritance и auxiliary runtime tables
`filament_hot_bed_nozzles.json`/`filament_compatibility.json`. Writer и GUI
должны использовать один и тот же exact pack; смешивание bundled и installed
bytes считается unsupported installation.

Для четырёх физических slots должны соблюдаться разные виды identity:

- `filament_settings_id[i]` хранит имя resolved filament profile по schema Orca;
- `filament_ids[i]` хранит `setting_id` leaf filament preset. Именно это
  значение пишет обычный **Save Project** в Snapmaker Orca 2.3.5;
- inherited material-family `filament_id` не подставляется в project array, но
  сохраняется отдельно от `setting_id` в loadout каждого artifact в
  `manifest.json` как `filamentId`; рядом сохраняются `profile` и `settingId`.

Validator проверяет exact profile hashes, closure inheritance, четыре значения
каждого project array, совпадение leaf `setting_id` и отсутствие source P1S
scripts/presets. GUI qualification дополнительно обязана доказать оба свойства:

1. candidate открывается без missing/custom-profile warning;
2. после save → close → reopen → re-save Snapmaker Orca не инъецирует embedded
   `settings_N.config` entries.

Если в будущем появится custom/embedded mode, он должен быть отдельной
capability со своей сериализацией, deduplication и qualification; Stage B system
profile mode автоматически не расширяется.

### WR-010. Stale artifacts

Output всегда считается unsliced. Не переносятся:

- `Metadata/plate_*.gcode`;
- `Metadata/plate_*.gcode.md5`;
- прежний slice cache;
- прежние time/weight/material estimates;
- `gcode_file` references;
- старые plate JSON и filament sequence, если они являются derived data;
- старые `plate_*`, `top_*`, `pick_*` previews после repack;
- cloud/device/calibration/network identifiers.

`slice_info.config` либо не создаётся, либо создаётся в adapter-approved header-only форме. Он не может утверждать, что plates уже нарезаны.

### WR-011. Thumbnails

На первом этапе допускаются два валидных режима:

1. writer генерирует новые thumbnails/previews и создаёт relationships;
2. writer не создаёт их и полностью удаляет соответствующие relationships/metadata references, если target adapter прошёл round-trip без них.

Копировать preview старой раскладки запрещено. Dangling thumbnail relationship является hard error.

### WR-012. Атомарная запись

Каждый artifact записывается во временный файл в destination directory, затем:

1. flush;
2. `fsync` файла;
3. structural validation;
4. SHA-256;
5. atomic rename.

Manifest публикуется последним. При cancel/error временные файлы удаляются, опубликованный ранее output не повреждается.

Статус: no-clobber atomic publication, cleanup на обычных error paths, явный
`cancel_conversion`, кооперативные checkpoints и startup recovery registry для
abandoned staging реализованы. Публикация mixed bundle начинается только после
сверки результата каждого adapter с утверждённым preflight contract и проверки
замкнутого дерева файлов без symlink/необъявленных вложений.

## 9. U1 Direct Spools writer

### 9.1. Physical slots

- T1–T4 имеют IDs 1–4;
- project arrays имеют adapter-defined cardinality для четырёх физических toolheads;
- physical color/material/profile order строго совпадает с plan loadout;
- `filament_ids` берётся из leaf `setting_id` каждого exact system profile;
  inherited material-family `filament_id` записывается отдельно в manifest
  loadout;
- один spool не может одновременно представлять несовместимые material profiles;
- explicit many-to-one palette reduction допускает назначение нескольких source requirements на одну физическую катушку.

### 9.2. Remap

Writer обновляет:

- object-level `extruder`;
- part/volume-level `extruder`;
- `paint_color` leaf states;
- `mmu_segmentation`, если присутствует;
- support/interface filament assignments;
- plate/tool metadata, если оно не derived.

Значение 0/unpainted обрабатывается по правилам исходного dialect и default extruder конкретного part; запрещена простая текстовая замена цифр.

### 9.3. Acceptance

U1 Direct output на Sailfin обязан:

- сохранить 3 428 075 vertices и 6 855 906 triangles;
- сохранить четыре printable instances;
- корректно remap все paint trees;
- показать ровно четыре физические катушки в заданном порядке;
- открыть и нарезаться в Snapmaker Orca 2.3.5;
- после save/reopen не менять plate membership и tool mapping.

### 9.4. Текущая реализация candidate

На Этапе B уже реализованы:

- exact gate для Snapmaker Orca 2.3.5, executable SHA-256 и allowlist hashes
  effective machine/process/filament profile inheritance из того же active
  application-data vendor pack, который загружает GUI;
- четыре physical slots и adapter-defined project arrays, включая resolved
  leaf `setting_id` в `filament_ids` и отдельный manifest `filamentId`;
- zero-embedded system-profile policy и validator, запрещающий generated
  process/filament/machine `settings_N.config`;
- explicit Textured PEI/by-layer/non-spiral/traditional-timelapse target globals
  и fail-closed проверка source per-plate process overrides с нормализацией
  только эквивалентных значений;
- Direct many-to-one mapping на уровне object, part и recursive paint tree;
- сохранение полного состава каждой source plate с изменением только
  target-grid translation;
- сохранение source `identify_id` каждого выбранного Orca instance;
- exact membership/transform/bounds validation для каждой planned instance;
- проверка Production Extension component/relationship closure;
- детерминированная multi-plate grid по контракту Snapmaker Orca 2.3.5;
- проверка U1 bed/Z bounds и prime tower от lower-left body anchor с actual
  printed-height cone, 45 мм depth, brim, rib, bed и object-clearance envelope;
- exact Orca `Auto For Flush` plate mapping с одной `1` на каждый из четырёх
  target filaments и staged-output validation mode/cardinality/plate IDs;
- сохранение Orca sinking/negative-Z semantics без lift с отклонением полностью
  находящейся ниже bed geometry;
- source snapshot + повторная source identity verification;
- strict unsliced structural validation и atomic no-clobber bundle publication.

Этот перечень структурных свойств дополнен валидной GUI qualification evidence.
На exact проверенной установке `conversionAvailable=true`; при любом
несовпадении executable/profile/report evidence capability закрывается.

Exact Sailfin candidate успешно прошёл native structural,
geometry-fingerprint, plate membership, slot-remap, target-settings и strict
bundle validation, затем два чистых GUI open/slice/save цикла. Candidate и оба
GUI saves имеют одинаковый semantic SHA-256 под version-scoped validator.

## 10. U1 Full Spectrum writer

### 10.1. Physical и virtual filaments

- физические T1–T4 занимают IDs 1–4;
- T1=C, T2=M, T3=Y, T4=X согласно плану;
- virtual mixed filament IDs начинаются после физических, то есть с 5;
- каждый virtual ID имеет одну каноническую definition и stable recipe fingerprint;
- одинаковые recipes дедуплицируются только при совпадении material, components, mode, ratios, subdivision и calibration context.

### 10.2. Serialized contract

U1 adapter сериализует `mixed_filament_definitions` и связанные project flags в точной форме целевой Snapmaker Orca. Definition должна включать как минимум:

- virtual filament ID и stable ID;
- display name и predicted color;
- component physical IDs;
- component order;
- mix ratios/weights;
- distribution mode;
- layer/subdivision parameters;
- gradient/pointillism/dithering parameters, если применимо;
- enabled/custom/origin flags;
- calibration and recipe fingerprint в manifest.

Object, part и paint assignments ссылаются на virtual IDs. Validator проверяет, что каждый virtual ID разрешается в definition, а каждая definition использует только физические IDs текущего loadout.

### 10.3. Capability gate

Production Full Spectrum включается только при валидной отдельной hash-bound
GUI evidence. Выполненный fixture:

- создан в Snapmaker Orca 2.3.5;
- содержит CMY+X physical profiles;
- содержит реально назначенный mixed recipe;
- содержит solid T4 region;
- прошёл slice → save → close → reopen → slice;
- зафиксировал exact serialized definitions и profile hashes.

Sailfin не удовлетворяет этому gate, поскольку не содержит virtual mixed
recipe. Поэтому Stage D квалифицирован отдельным шестиплатным кандидатом из
Withered Foxy. Schema v2 квалифицирует только native writer structure и GUI
interoperability. Она намеренно не содержит `physicalCouponPassed`: точность
recipe проверяется per-user measured `calibrationSampleId` для exact loadout
либо явным fingerprint-bound color approval, который остаётся nominal, а не
measured.

## 11. A1 mini writer без AMS

### 11.1. Output policy

- один physical spool;
- один material profile;
- один effective logical slot;
- все object/part/paint assignments сводятся к slot 1 только после явного решения плана;
- `has_filament_switcher=0`;
- AMS Lite не требуется;
- presence полей с `AMS` в имени не используется для вывода об активности AMS;
- adapter сериализует их в точности как qualified target schema.

### 11.2. Material safety

Если source unit использует PETG, A1 output остаётся PETG, пока пользователь явно не подтвердил material substitution. Один только выбор серой/чёрной PLA-катушки не является таким подтверждением.

Manifest обязан хранить:

- source material;
- target material;
- physical spool ID;
- target filament profile;
- отдельный approval ID для замены polymer family;
- предупреждение о механических свойствах.

### 11.3. Первоначальный plate mode

Первая production-версия создаёт один A1 3MF на одну target plate. Это исключает зависимость от незафиксированной виртуальной multi-plate grid Bambu Studio.

Multi-plate A1 writer добавляется после clean two-plate fixture и проверки:

- grid pitch/origins;
- build transforms;
- model settings membership;
- thumbnails;
- save/reopen stability.

### 11.4. Acceptance

Каждый A1 output должен:

- содержать только назначенные source units;
- сохранять их vertex/triangle counts, scale и orientation;
- помещаться в 180 × 180 × 180 мм с требуемыми clearance;
- показывать A1 mini 0.4 nozzle и один внешний spool;
- открываться без repair/profile warning;
- успешно проходить Slice Plate;
- после save/reopen сохранять material, placement и spool assignment.

## 12. Structural validator

Validator выдаёт `valid`, `warning` или `error` по структурированным codes.

Обязательные проверки:

1. ZIP signature, CRC, paths, duplicate entries, encryption, symlinks,
   compression ratios, однозначный EOCD/ZIP64 graph и advertised entry count до
   создания `ZipArchive`;
2. `[Content_Types].xml` closure;
3. существование каждого internal relationship target;
4. XML well-formedness, namespace и запрет DTD/entities;
5. JSON schema/type validation;
6. уникальность object IDs и UUID в допустимом scope;
7. Production Extension relationship closure;
8. отсутствие dangling component/object/build references;
9. каждый planned source unit ровно в одном output location;
10. совпадение geometry counts/fingerprints;
11. сохранение scale/rotation и допустимость только planned translation;
12. plate membership/build transform consistency;
13. bounds, bed polygon, clearance, collision и height;
14. target printer/nozzle/profile identity;
15. project setting array cardinality по adapter-specific schema;
16. physical и virtual filament reference closure;
17. material/profile compatibility;
18. отсутствие source P1S machine G-code в target;
19. отсутствие sliced artifacts;
20. отсутствие dangling thumbnail references;
21. source SHA-256 не изменился.

Hard error запрещает публикацию соответствующего artifact. Warning отображается пользователю и записывается в manifest, но не блокирует, если это явно разрешённый warning code.

## 13. Native slicer qualification

Structural validity недостаточна. Для каждого adapter создаётся qualification matrix:

| Target | Fixture | Проверка |
|---|---|---|
| U1 Direct | solid + painted Sailfin-derived public fixture | open, Slice All, save, close, reopen, Slice All |
| U1 Full Spectrum | physical CMY+X + mixed + solid T4 | open, inspect virtual colors, slice, save, reopen, slice |
| A1 PLA | clean one-plate fixture | open, Slice Plate, save, reopen, slice |
| A1 PETG | clean one-plate fixture | open, Slice Plate, save, reopen, slice |
| A1 multi-plate | clean two-plate fixture | требуется только для capability `a1_multi_plate` |

CLI может запускаться как дополнительный smoke test, но не является единственным доказательством. Текущие CLI experiments показали version/profile mismatch и crash/error paths на новых samples.

### 13.1. Построение U1 Direct qualification candidate

Candidate создаётся только отдельным example entry point. Он намеренно не
подключён к desktop production command:

```sh
mkdir -p "/path/to/qualification-output"
cargo run -p u1-application --example u1_direct_qualification -- \
  "Sample/Sailfin Dragon - Articulated Lizard by Raki-Box.3mf" \
  "/path/to/qualification-output" \
  "/Applications/Snapmaker Orca.app"
```

Команда допускает bypass только для отсутствующей GUI qualification, но не для
wrong application version, executable hash, profile hash, planner error,
structural error или source identity mismatch. Результат остаётся
qualification artifact и не должен выдаваться пользователю как production
conversion.

### 13.2. Обязательная GUI-процедура U1 Direct

Для каждого candidate на exact Snapmaker Orca 2.3.5:

1. вычислить SHA-256 writer candidate до открытия;
2. открыть 3MF, подтвердить `Snapmaker U1 (0.4 nozzle)`, plate membership,
   T1–T4 colors/materials/profiles, Textured PEI/by-layer/non-spiral/traditional
   target globals и отсутствие repair/incompatible-profile warning,
   missing-profile warning и custom-profile warning;
3. выполнить `Slice All`, проверить tool mapping и фактический prime tower;
   tower должен соответствовать lower-left anchor и оставаться в пределах
   рассчитанного cone/depth/brim/rib/clearance envelope;
4. выполнить `Save As` в первый новый 3MF и вычислить его SHA-256;
5. закрыть проект и приложение;
6. заново открыть первый GUI-saved 3MF, повторно проверить plates и T1–T4;
7. повторно выполнить `Slice All`, затем `Save As` во второй новый 3MF и
   вычислить его SHA-256;
8. структурно проверить оба GUI-saved файла: ни один не должен получить
   process/filament/machine `settings_N.config`; `filament_ids`, profile names и
   derived identity/`inherits` resolution, target globals, plate membership,
   семантическое соответствие instances и prime-tower coordinates должны
   остаться совместимыми и стабильными. Writer candidate обязан точно сохранять
   source `identify_id`; обычный GUI Save может присвоить новые process-local
   числа, поэтому для GUI-файлов проверяются положительность, уникальность,
   внутренняя согласованность и однозначное соответствие по geometry/name/
   transform/bounds/extruder/plate membership;
9. сохранить отдельный qualification report с результатом каждого шага,
   видимыми warnings и hashes, затем вычислить SHA-256 самого report;
10. только после независимой проверки заполнить встроенную qualification-запись.

Headless CLI run может дополнять report, но не заменяет ни один GUI-шаг. Crash
CLI сам по себе не означает дефект candidate: для квалификации используется
сравнение GUI-поведения candidate и официального source fixture в одной exact
установке.

### 13.3. Qualification record и evidence

Production gate читает два независимых файла как строгие typed schemas без
неизвестных полей:

- `crates/orca-adapter/qualification/u1-direct-2.3.5.json` — release record;
- `crates/orca-adapter/qualification/u1-direct-2.3.5-report.json` — подробный
  hash-bound qualification report.

Оба файла заполнены exact qualification evidence и имеют `qualified` status.
Release record имеет следующую форму:

```json
{
  "schemaVersion": 2,
  "adapterId": "snapmaker-orca/2.3.5/u1-0.4-direct",
  "applicationVersion": "2.3.5",
  "executableSha256": "4c30e59cf582dcc4f12e43741fcab2f97045e972065d465481ed3760678d0fbe",
  "profileSource": "installed_system",
  "profilePackVersion": "02.02.53.02",
  "profileManifestSha256": "08d2e3a4450f07fa75f123c691495b3cd3c354c3c29697ceb8a35d6407186cb9",
  "guiRoundTripPassed": true,
  "status": "qualified",
  "evidence": {
    "fixtureName": "Sailfin Dragon - Articulated Lizard by Raki-Box.3mf",
    "sourceFixtureSha256": "1b20d6124353d3bc31c4ea554e482ba6f8ef281dd23e2df6bbc0d561b4f6e0d7",
    "writerCandidateSha256": "<64-hex SHA-256>",
    "firstGuiSavedSha256": "<64-hex SHA-256>",
    "reopenedGuiSavedSha256": "<64-hex SHA-256>",
    "qualificationReportSha256": "<64-hex SHA-256>",
    "opened": true,
    "sliced": true,
    "saved": true,
    "closed": true,
    "reopened": true,
    "resliced": true,
    "resaved": true,
    "qualifiedAtUtc": "<non-empty UTC timestamp>"
  },
  "note": "<non-empty qualification note>"
}
```

Отдельный report имеет следующую typed форму:

```json
{
  "schemaVersion": 2,
  "adapterId": "snapmaker-orca/2.3.5/u1-0.4-direct",
  "applicationVersion": "2.3.5",
  "executableSha256": "4c30e59cf582dcc4f12e43741fcab2f97045e972065d465481ed3760678d0fbe",
  "profileSource": "installed_system",
  "profilePackVersion": "02.02.53.02",
  "profileManifestSha256": "08d2e3a4450f07fa75f123c691495b3cd3c354c3c29697ceb8a35d6407186cb9",
  "status": "qualified",
  "qualification": {
    "fixtureName": "Sailfin Dragon - Articulated Lizard by Raki-Box.3mf",
    "sourceFixtureSha256": "1b20d6124353d3bc31c4ea554e482ba6f8ef281dd23e2df6bbc0d561b4f6e0d7",
    "writerCandidateSha256": "<64-hex SHA-256>",
    "firstGuiSavedSha256": "<64-hex SHA-256>",
    "reopenedGuiSavedSha256": "<64-hex SHA-256>",
    "opened": true,
    "sliced": true,
    "saved": true,
    "closed": true,
    "reopened": true,
    "resliced": true,
    "resaved": true,
    "qualifiedAtUtc": "<non-empty UTC timestamp>",
    "profiles": [
      {
        "relativePath": "<exact DIRECT_PROFILE_BASELINE path>",
        "sha256": "<exact baseline SHA-256>"
      }
    ],
    "auxiliaryProfiles": [
      {
        "relativePath": "<exact DIRECT_AUXILIARY_PROFILE_BASELINE path>",
        "sha256": "<exact baseline SHA-256>"
      }
    ],
    "checks": {
      "machineProfileLoaded": true,
      "processProfileLoaded": true,
      "t1T4MappingVerified": true,
      "primeTowerVerified": true,
      "firstGuiSavedStructurallyValid": true,
      "reopenedGuiSavedStructurallyValid": true,
      "geometryAndPlacementsStable": true,
      "targetGlobalsStable": true,
      "writerCandidateIdentifyIdsPositiveAndUnique": true,
      "writerCandidateSourceIdentifyIdsPreserved": true,
      "guiIdentifyIdsUnique": true,
      "instanceIdentityBijectionStable": true,
      "plateCountStable": true,
      "objectCountStable": true,
      "sliceCompletedWithoutRepairWarning": true,
      "noIncompatibleProfileWarning": true,
      "noMissingProfileWarning": true,
      "noCustomProfileWarning": true,
      "noEmbeddedPresetsAfterFirstSave": true,
      "noEmbeddedPresetsAfterReopenedSave": true,
      "derivedProfileIdentityStable": true
    }
  },
  "note": "<non-empty qualification note>"
}
```

Все четыре generated evidence hashes (`writerCandidate`, два GUI-saved файла и
qualification report) должны быть 64-символьными hex SHA-256 и не могут быть
нулевыми. Fixture name/hash, adapter ID, version, executable hash, effective
profile source, pack version и manifest hash должны точно совпасть со встроенным
baseline. Report должен перечислить все exact profile и auxiliary runtime
path/hash пары в том же количестве и порядке, что и `DIRECT_PROFILE_BASELINE` и
`DIRECT_AUXILIARY_PROFILE_BASELINE`; все typed checks и семь step flags
обязательны.
`qualificationReportSha256` в record должен совпасть с SHA-256 точных bytes
report, а общие evidence fields двух документов должны быть идентичны. Поэтому
изменение только `guiRoundTripPassed` или `status` не разблокирует writer.

## 14. UI и пользовательский поток

### 14.1. Состояния кнопки

`Approve & Convert` активна только если:

- анализ относится к текущему source SHA-256;
- plan пересчитан после последних изменений;
- `planReady=true`;
- нет omitted mandatory units;
- все material/color approvals действительны;
- packing validation пройдена;
- для каждого output job есть qualified adapter;
- destination ещё не выбран либо может быть безопасно выбран при нажатии.

Если writer ещё не подключён, UI должен показывать конкретно:

```text
Conversion is unavailable: no qualified writer adapter is installed.
```

Фраза `Writer command not connected` заменяется на actionable capability status.

### 14.2. Preflight dialog

После нажатия:

1. выбрать destination directory;
2. показать список будущих файлов;
3. показать printer, plate count, material и loadout каждого файла;
4. показать точные T1–T4 setup actions;
5. показать, что output необходимо открыть и нарезать;
6. показать warnings и размеры;
7. запросить подтверждение `Convert`.

### 14.3. Progress

Этапы с progress events:

```text
Verifying source
Preparing target profiles
Packing plates
Copying geometry
Remapping colors
Writing project metadata
Validating output
Publishing files
```

Cancel разрешён до atomic publication. Backend передаёт progress events,
проверяет cancel между ограниченными фазами и не публикует частичный bundle.
После crash/kill recovery registry позволяет безопасно удалить только staging,
принадлежащий завершившемуся процессу.

### 14.4. Result screen

Для каждого artifact показываются:

- filename;
- target printer;
- plates;
- physical loadout/spool;
- SHA-256;
- structural validation status;
- `Open in Snapmaker Orca` либо `Open in Bambu Studio`;
- `Show in Finder`;
- напоминание `Slice and verify tool mapping before printing`.

Автоматически открывать слайсер без действия пользователя не требуется.

## 15. Native API

Реализованные mixed-conversion Tauri commands:

```text
inspect_conversion_capabilities()
prepare_conversion(source_path, source_sha256, plan_fingerprint)
convert_project(preparation_token, destination_directory)
cancel_conversion(conversion_id)
```

Они принимают только backend-canonical analysis/plan. Preparation token
одноразовый: frontend validation, destination checks и source checks выполняются
до его потребления. Conversion разрешается только при валидной hash-bound GUI
qualification record и совпадении exact local installation evidence.

Отдельные удобства, не являющиеся writer-командами:

```text
validate_output(path, adapter_id)
open_output_in_slicer(path, adapter_id)
```

Требования:

- `preparation_token` короткоживущий и связан с source hash, canonical plan и adapter evidence;
- destination проходит canonicalization и scope checks;
- backend игнорирует frontend-generated plate/job JSON;
- progress передаётся Tauri events;
- native errors имеют stable English code и user-facing English message;
- logs не содержат geometry, cloud IDs, serial numbers или абсолютные пути без debug opt-in.

Текущий CLI предоставляет `analyze`, `plan`, Direct-aware `doctor`,
`validate-output` и `validate-u1-gui-round-trip`:

```text
u1-converter doctor
u1-converter validate-output <file>
u1-converter validate-u1-gui-round-trip --source <fixture> <candidate> <save-1> <save-2>
```

Отдельный production CLI `convert --source <file> --plan <file> --output
<directory>` остаётся целевым API; desktop использует уже реализованные Tauri
commands.

## 16. Производительность и безопасность

### 16.1. Type-aware limits

Нельзя просто убрать ограничение размера. Требуется разделить limits:

- metadata/config/relationships — малые строгие limits;
- model XML — отдельный streaming limit не менее 1 GiB на entry для U1 Sailfin;
- total uncompressed model data — отдельный bounded limit;
- общий entry count;
- max XML depth;
- max attributes/text token;
- max paint annotation length;
- max object/vertex/triangle counts;
- compression ratio с отдельной политикой для model XML и metadata.

До изменения defaults Sailfin должен проходить только явным test profile; после реализации type-aware policy он должен проходить стандартный production analyzer.

### 16.2. Performance targets

На текущем reference Mac в release build:

- повторный plan не перечитывает geometry;
- `Withered_Foxy.3mf` analysis target ≤ 10 секунд;
- A1 sample analysis target ≤ 12 секунд;
- U1 monolithic Sailfin target ≤ 25 секунд с видимым progress;
- peak memory не превышает 1 GiB для любого из трёх samples;
- conversion не хранит все mesh XML одновременно в памяти;
- hash, copy и validation по возможности объединяются в один streaming pass.

Превышение target не является corruption error, но фиксируется benchmark regression test.

## 17. Тестовая стратегия

### 17.1. Unit tests

- OPC path normalization и relationships;
- Content Types builder;
- deterministic IDs/UUIDs;
- target setting allowlist;
- explicit U1 target globals и equivalent/conflicting per-plate overrides;
- array cardinality per adapter;
- embedded preset deduplication;
- object/part/facet remap;
- recursive paint tree many-to-one remap;
- Full Spectrum virtual ID allocation;
- A1 single-slot collapse;
- material substitution approvals;
- stale artifact filter;
- source `identify_id` preservation;
- positive/unique/internal GUI instance IDs и semantic instance bijection между
  candidate и двумя GUI saves;
- prime-tower lower-left anchor и printed-height cone/depth/brim/rib/clearance
  envelope;
- typed qualification record/report cross-binding и report-byte SHA-256;
- atomic writer/cancel behavior;
- manifest source-to-target closure.

### 17.2. Property and mutation tests

- decode → remap → encode paint round-trip;
- arbitrary safe OPC graphs;
- duplicate/dangling relationship mutations;
- invalid UUID/ID references;
- truncated ZIP/XML/JSON;
- ZIP bombs и oversized metadata;
- plate duplication/omission;
- illegal transform changes;
- invalid physical/virtual filament IDs;
- stale G-code injection.

### 17.3. Sample contracts

#### Sailfin

- стандартный analyzer принимает monolithic 619+ MiB model XML;
- counts совпадают;
- Direct writer сохраняет geometry и remap semantics;
- exact hardened candidate проходит native structural, geometry, membership,
  target-setting и bundle validators;
- обязательный Snapmaker Orca GUI round-trip выполнен, два GUI saves прошли
  version-scoped structural/semantic validation.

#### Withered Foxy source

- все 89 objects/instances и 390 parts представлены в manifest;
- ни один mandatory unit не пропущен и не продублирован;
- output bundle сохраняет 7 308 333 vertices и 14 616 548 triangles;
- PLA/PETG не объединяются без approval;
- плановые U1/A1 assignments совпадают с canonical plan.

#### A1 reference

- schema parser понимает one-spool/no-AMS representation;
- 9-plate файл используется только как forensic comparison;
- тест обязан обнаружить известные scale changes и PETG→PLA substitution;
- файл не может получить статус qualified golden.

### 17.4. UI tests

- disabled reason для каждого capability gate;
- preflight file/loadout list;
- no conversion from stale plan;
- cancel leaves no final artifacts;
- per-artifact success/error state;
- correct slicer launch target;
- progress remains responsive on large source;
- no output overwrite without confirmation.

## 18. Этапы реализации

### Этап A. Package writer и validator

- type-aware streaming limits;
- OPC/ZIP builder;
- Production Extension writer;
- relationship/content-type validator;
- stale artifact filter;
- deterministic manifest;
- atomic publication.

Результат: generic unsliced Project 3MF package без printer-specific remap.

Статус: реализовано. Добавлены type-aware analyzer/writer/validator limits,
включая лимит одной XML lexical token, приватный immutable snapshot исходника,
детерминированный OPC/ZIP staging, Production Extension builder и двусторонняя
relationship closure validation, namespace-aware Production attributes,
поддержка direct build-item path и root-level relationship parts, обязательная
повторная проверка source size/SHA-256 через исходный handle,
stale artifact policy, manifest, привязанный к validated staging capability и
проверенному source identity, fixed strict-unsliced gate и no-clobber атомарная
публикация из закрытого validated snapshot в приватном каталоге `0700`.
Сквозной synthetic test выполняет цепочку
`build → stage → validate → publish → analyze`. Printer-specific settings,
packing и color/material remap намеренно не входят в этот этап.

### Этап B. U1 Direct Spools

- U1 2.3.5 target config resolver — реализован с exact executable/profile
  hashes и immutable resolved profile store из active application-data vendor
  pack;
- four-slot arrays — реализованы с `filament_ids=leaf setting_id` и отдельным
  material-family `filamentId` в manifest;
- exact system-profile/zero-embedded policy — реализована и проверяется
  structural validator;
- typed release record + separate exact-byte-hash-bound qualification report
  gate — реализован; оба документа qualified и взаимно hash-bound;
- sinking/negative-Z preservation без lift — реализовано с hard error для
  полностью находящегося ниже bed объекта;
- Textured PEI/by-layer/non-spiral/traditional target globals и safe per-plate
  override normalization — реализованы;
- lower-left/printed-height prime-tower envelope и source `identify_id`
  preservation — реализованы;
- plate/build/model settings writer — реализован;
- object/part/paint remap — реализован;
- semantic validation source plate/unit → target plate/artifact — реализована;
- strict bundle publication — реализована;
- hardened Sailfin candidate — построен, прошёл native validators и два чистых
  GUI open/slice/save цикла;
- Sailfin GUI qualification — выполнена 3 августа 2026 года, candidate и оба
  GUI saves имеют один semantic SHA-256;
- UI capability `u1_direct` — подключён и сообщает `qualified` на exact
  проверенной установке, разрешая conversion для валидного canonical plan.

Целевой результат: первая реально работающая `Approve & Convert` для
solid/Direct U1 jobs. Этот результат достигнут для exact qualified установки;
при несовпадении executable/profile evidence кнопка остаётся выключенной.

Завершённые действия qualification Этапа B:

1. выполнен и задокументирован exact GUI round trip из раздела 13, заполнен
   typed report и его exact-byte SHA-256 привязан к release record;
2. доказана в GUI совместимость derived profile identity groups и `inherits`
   chains, отсутствие missing/custom-profile warning и embedded entry injection
   после обоих re-save;
3. повторно пройдены structural, semantic, sample, workspace и UI test suites
   после установки qualification evidence.

Post-qualification hardening Этапа B, включая cooperative cancel,
abandoned-staging recovery и mixed-bundle publication, реализован.

### Этап C. A1 mini single-plate mono

- clean PLA/PETG fixtures exact Bambu Studio version;
- target adapter;
- production packer 180 × 180;
- one-slot material writer;
- single-plate output;
- A1 round-trip;
- UI capability `a1_mono_single_plate`.

Статус: native single-plate writer и target adapter реализованы. Writer
создаёт один independently packed A1 mini Project 3MF на target plate, ровно с
одной внешней PLA/PETG катушкой и без AMS semantics. Отдельные PLA и PETG
candidate прошли полный GUI-цикл в exact Bambu Studio 02.02.00.85. Strict
validator подтвердил geometry/topology, parts, transforms, placements,
machine/process/material/color identity; typed report и release record
hash-bound, production gate имеет статус `qualified`.

### Этап D. U1 Full Spectrum

- GUI fixture с mixed definition;
- schema serialization;
- virtual filament remap;
- recipe/profile fingerprints;
- Full Spectrum round-trip;
- отдельная per-user physical color calibration;
- UI capability `u1_full_spectrum`.

Статус: writer и schema adapter реализованы. Они сериализуют физические CMY+X,
virtual mixed definitions, decoded paint assignment, solid T4 regions,
subdivision/process globals и active prime tower. Шестиплатный candidate прошёл
полный Snapmaker Orca GUI-цикл; strict validator подтвердил эти данные,
geometry/parts/transforms/placements и profile identity между candidate и двумя
GUI save. Writer gate имеет статус `qualified`. Физическая точность цвета
намеренно отделена: measured результат требует per-user calibration provenance
для exact loadout, а явное color approval остаётся nominal.

### Этап E. A1 multi-plate и hardening

- clean two-plate fixture;
- virtual grid contract;
- multi-plate thumbnails;
- mutation/fuzz/performance tests;
- installer/version diagnostics.

## 19. Критерии приёмки

Функция считается готовой, когда одновременно выполнены условия:

1. `Approve & Convert` создаёт не JSON-план, а реальные target Project 3MF;
2. source hash до и после совпадает;
3. все mandatory source units присутствуют ровно один раз;
4. geometry counts, scale и orientation сохранены;
5. только target plate translations отличаются от source;
6. U1 output содержит ровно соответствующий batch loadout;
7. A1 output содержит один spool, один material profile и no-AMS semantics;
8. Full Spectrum definitions и virtual assignments совпадают с утверждёнными recipes;
9. material substitutions возможны только с сохранённым explicit approval;
10. structural validator не находит dangling references или stale slice artifacts;
11. каждый artifact открывается без repair dialog и incompatible-printer warning;
12. все plates успешно нарезаются в exact target slicer;
13. save → close → reopen → slice не меняет состав, scale, material и tool mapping;
14. output публикуется атомарно, cancel/error не оставляет частично готовый bundle;
15. manifest позволяет однозначно восстановить source → output → plate → spool;
16. UI показывает, какой файл открыть, какие катушки загрузить и что проверить перед печатью.

На 3 августа 2026 года пункты 1–16 реализованы программно для mixed Direct /
Full Spectrum / A1 output; exact-partition validator отдельно доказывает пункт
3. Этапы B–D закрывают GUI-пункты 11–13 своими независимыми qualified
матрицами. Физическая цветовая калибровка Full Spectrum остаётся per-user
evidence и не входит в глобальную qualification структуры writer.

## 20. Обязательные fixtures до production release

Уже имеются:

- сложный Bambu source fixture;
- реальный U1 painted project reference;
- реальный A1 mini no-AMS forensic reference.

Текущий внутренний Stage B qualification gate привязан по имени и SHA-256 к
предоставленному Sailfin reference. Это позволяет выполнить локальную
квалификацию writer, но не даёт права распространять сам пользовательский файл
как публичный test asset.

Для текущих single-plate A1, U1 Direct и U1 Full Spectrum writer обязательная
локальная GUI evidence получена. Для будущего `a1_multi_plate` всё ещё нужен
отдельный чистый A1 mini two-plate fixture и собственный capability gate.

Заполненные Direct, Full Spectrum и A1 mini typed reports содержат exact
profile, candidate и GUI-saved evidence, а соответствующие release records —
SHA-256 точных bytes reports. Все три текущих writer gate имеют `qualified`
status.

Предоставленные пользовательские models нельзя переносить в публичные test fixtures.

## 21. Лицензионное ограничение

Snapmaker OrcaSlicer, OrcaSlicer и Bambu Studio распространяются по GNU AGPL v3. Их source используется для изучения наблюдаемого формата и поведения. В proprietary workspace запрещено:

- копировать их C++ exporter;
- статически или динамически связывать его с приложением;
- переносить большие фрагменты implementation logic;
- скрыто распространять изменённый AGPL-компонент.

Выбранная реализация — самостоятельный Rust writer по открытой спецификации 3MF, собственным fixtures и clean-room tests. Это техническое решение, а не юридическая консультация.

## 22. Официальные источники

- [Snapmaker OrcaSlicer repository](https://github.com/Snapmaker/OrcaSlicer)
- [Snapmaker Project 3MF API (`bbs_3mf.hpp`, v2.3.5)](https://github.com/Snapmaker/OrcaSlicer/blob/v2.3.5/src/libslic3r/Format/bbs_3mf.hpp)
- [Snapmaker Project 3MF writer (`bbs_3mf.cpp`, v2.3.5)](https://github.com/Snapmaker/OrcaSlicer/blob/v2.3.5/src/libslic3r/Format/bbs_3mf.cpp)
- [Snapmaker plate serialization (`PartPlate.cpp`, v2.3.5)](https://github.com/Snapmaker/OrcaSlicer/blob/v2.3.5/src/slic3r/GUI/PartPlate.cpp)
- [Snapmaker Full Spectrum definitions (`MixedFilament.cpp`)](https://github.com/Snapmaker/OrcaSlicer/blob/v2.3.5/src/libslic3r/MixedFilament.cpp)
- [Bambu Studio repository](https://github.com/bambulab/BambuStudio)
- [Bambu Studio CLI documentation](https://github.com/bambulab/BambuStudio/wiki/Command-Line-Usage)
- [3MF Core Specification](https://github.com/3MFConsortium/spec_core/blob/master/3MF%20Core%20Specification.md)
- [3MF Production Extension](https://github.com/3MFConsortium/spec_production/blob/master/3MF%20Production%20Extension.md)
- [OrcaSlicer Project 3MF import/export notes](https://github.com/OrcaSlicer/OrcaSlicer/wiki/import_export)
