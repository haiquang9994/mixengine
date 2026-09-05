+++
title = "Phiên bản PHP, Node, Python và Ruby"
slug = "runtimes"
order = 5
summary = "Cài bao nhiêu phiên bản tùy bạn, và để mỗi thư mục tự chọn phiên bản của nó — không hook shell, không phải nhớ gì cả."
translation_of = "en/runtimes.md"
source_sha256 = "f1f3b60d4bef5176ea6682af42816d1139eca0413fd9755eb20921ce223c7ce6"
+++

# Phiên bản PHP, Node, Python và Ruby

MixEngine cài các runtime ngôn ngữ vào thư mục của riêng nó, mỗi phiên bản một thư mục bất biến, và
không bao giờ đụng tới thứ mà hệ điều hành của bạn đã có sẵn. Cài một phiên bản không bao giờ sửa
một phiên bản đã cài, nên không thứ gì đang chạy được của bạn bị hỏng vì bạn thêm một thứ mới.

Có bốn ngôn ngữ được quản lý: **PHP**, **Node.js**, **Python** và **Ruby**.

## Cài một phiên bản

```bash
mix runtime available --kind php
mix runtime install php 8.3.33
mix runtime list
```

Phiên bản phải chính xác, và đó là chủ ý chứ không phải thiếu sót. `8.3` là câu *"chọn giùm tôi một
cái"*, mà chưa có gì để chọn cho tới khi có thứ được cài — chọn giữa các phiên bản là việc của phân
giải, và phân giải trả lời bằng những gì đang có trên máy. `mix runtime available` mới là nơi dành
cho một khoảng.

Một lần cài là một job, và `mix` mặc định chờ nó: `mix runtime install php 8.3.33 && …` là một câu
nói về việc PHP đã có mặt. `--no-wait` trả về ngay khi daemon nhận việc và đưa cho bạn một id job,
mà `mix job wait` có thể trỏ tới sau.

**Cài một bản PHP cũng tạo ra pool php-fpm của nó** — `php-fpm@8.3.33`, một service như mọi service
khác, có trong `mix service list`. Node, Python và Ruby được gọi theo từng lệnh và không có gì được
giám sát.

### Trên máy Windows dùng chip ARM

Một số phiên bản không có bản dựng cho loại chip đó — chẳng hạn không ai phát hành PHP ARM64 cho
Windows cả. Ở những chỗ như vậy, MixEngine cài bản x86_64 và Windows chạy nó giúp bạn. Nó hoạt động
được; chỉ chậm hơn một chút so với một bản dựng riêng cho máy bạn.

Bạn không bao giờ phải tự đoán bản nào là bản nào. Trên máy đó, `mix runtime available` và
`mix package available` có thêm một cột `RUNS`, ghi `native` hoặc `emulated` cho từng phiên bản, và
lệnh cài nói ra điều đó trước khi bắt đầu tải. Trên mọi máy khác không có cột này, vì không có gì để
nói.

## Chọn phiên bản cho một thư mục

Không có gì ở đây đổi shell của bạn, vá một profile, hay bắt bạn gõ một lệnh kích hoạt. Một thư mục
phân giải ra một phiên bản, và các shim lo phần còn lại.

```bash
mix runtime default php 8.3.33      # mặc định cho cả máy
mix project update blog --pin php=^8.1
mix runtime resolve php             # *thư mục này* nhận cái nào, và vì sao?
```

`mix runtime resolve` là lệnh đáng nhớ. Nó trả lời đúng thứ `php -v` sẽ trả lời, mà không chạy gì
cả, **và** nó nêu tên cái nào trong bốn nguồn đã quyết định:

1. Một cờ hoặc biến môi trường tường minh trên chính câu lệnh đang chạy.
2. File `mixengine.toml` gần nhất có nhắc tới ngôn ngữ này, đi ngược lên từ chỗ bạn đứng.
3. Project đã đăng ký bao phủ thư mục này.
4. Mặc định toàn cục.

Một `mixengine.toml` không nói gì về PHP thì không phải câu trả lời về PHP, nên một pin ở trên vẫn
có hiệu lực.

### Viết một ràng buộc phiên bản

Pin và `--version` nhận ba dạng, tất cả đều được phân giải dựa trên các phiên bản **đã cài** chứ
không bao giờ âm thầm dựa trên những phiên bản có thể tải về:

| Viết | Nghĩa là |
| --- | --- |
| `8.3.33` | Đúng phiên bản đó |
| `8.3` hoặc `8` | Viết bao nhiêu đoạn thì bấy nhiêu đoạn phải khớp; đoạn không ai viết là số 0 |
| `^8.3` | Tới đoạn khác 0 ngoài cùng bên trái — `^0.12` dừng trước `0.13` |

Một ràng buộc không có phần tiền phát hành thì không bao giờ chọn một bản tiền phát hành. `8.5` và
`^8.5` đều bỏ qua `8.5.0RC1`; gọi đúng tên nó là cách bạn yêu cầu nó.

## Các shim

`mix path install` lấp đầy `<root>/bin` và đưa đúng một thư mục đó vào `PATH` của bạn. Nó chứa một
chương trình nhỏ cho mỗi lệnh — `php`, `php-config`, `pecl`, `composer`, `node`, `npm`, `npx`,
`python`, `pip`, `ruby`, `gem`, `bundle` — và mỗi cái tự tìm ra phiên bản mà thư mục này muốn rồi
chuyển giao cho chương trình thật.

Hai hệ quả đáng biết:

- **Nó chạy được khi daemon đang tắt.** Một shim đọc thẳng thứ nó cần chứ không hỏi qua socket, và
  đó là lý do `php -v` trong một project vẫn trả lời khi MixEngine không chạy.
- **Không có gì phải làm mới sau khi cài.** Danh sách lệnh là cố định, nên `<root>/bin` không phụ
  thuộc vào những gì bạn đã cài. Một shim `node` trên máy không có Node.js thì không phân giải ra gì
  cả và nói cho bạn biết cần gõ lệnh nào.

Chỉ `<root>/bin` được đưa vào `PATH` — đúng một mục, không bao giờ mỗi phiên bản một thư mục.

```bash
mix path status
mix path uninstall
```

`mix path uninstall` gỡ thư mục đó khỏi `PATH` và để các lệnh nguyên tại chỗ: chúng nằm trong home
của chính MixEngine, và gỡ home mới là thứ gỡ chúng đi.

## Extension của PHP

Extension gắn với từng phiên bản đã cài, vì chúng được biên dịch theo phiên bản đó:

```bash
mix runtime ext list --php 8.3.33
mix runtime ext enable redis --php 8.3.33
mix runtime ext disable xdebug --php 8.3.33
```

`list` cho biết bản dựng có những extension nào và **vì sao mỗi cái đang bật hay tắt**, đó thường
mới là câu hỏi. Bỏ `--php` đi nghĩa là phiên bản mà thư mục này phân giải ra.

Bật một extension sẽ nạp nó trên mọi tiến trình PHP của phiên bản đó, kể cả pool.

## Gỡ một phiên bản

```bash
mix runtime uninstall php 8.1.31
```

Lệnh này bị từ chối khi còn một project đã đăng ký ghim phiên bản đó — các project sẽ được nêu tên —
và khi pool php-fpm chạy trên nó đang chạy. `--force` vượt qua điều kiện thứ nhất và không bao giờ
vượt qua điều kiện thứ hai.
