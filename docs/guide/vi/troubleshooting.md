+++
title = "Khi có gì đó không ổn"
slug = "troubleshooting"
order = 14
summary = "mix doctor trước, rồi bốn câu lệnh trả lời đúng những câu hỏi người ta thật sự có — và một file chứa mọi thứ một báo cáo lỗi cần."
translation_of = "en/troubleshooting.md"
source_sha256 = "9830ff64602fc98102c0537ed6ef863f9edd8da7ec8f70da6ae8123de789598c"
+++

# Khi có gì đó không ổn

## Bắt đầu ở đây

```bash
mix doctor
```

Nó khám cỗ máy và nói cái gì đang sai. Nó **không báo cáo và không sửa gì** trừ khi bạn yêu cầu, và
nó thoát với mã khác 0 khi tìm thấy vấn đề, để một script cũng hỏi được.

```bash
mix doctor --repair
```

Sửa mọi thứ sửa được. Bất cứ gì nằm trong home của chính MixEngine được sửa ngay; bất cứ gì cần
quyền quản trị được xếp hàng, cho bạn xem, rồi được cấp trong **một** hộp thoại cho cả lô. `--yes`
bỏ qua bước xác nhận trước hộp thoại đó.

## Bốn câu hỏi người ta thật sự có

### "Có gì đang chạy không?"

```bash
mix status
mix service list
```

`status` nói về daemon: phiên bản, home, và nó đang giám sát gì. `service list` là mỗi service một
dòng, kèm việc mỗi cái đang làm gì.

### "Vì sao tên này không mở được?"

```bash
mix domain status blog.test
```

Bốn sự kiện, được trả lời tách rời chứ không gộp thành một phán quyết: tên đó đã được khai báo chưa,
nó được định tuyến ra sao, ngay lúc này nó có phân giải trên máy này không, và có gì trả lời trên nó
không. Cái nào là `không` thì đó là cái cần sửa.

### "Vì sao ổ khóa không xanh?"

```bash
mix cert status
mix cert ca-status
```

`cert status` mở một kết nối thật và báo lại chứng chỉ đã thật sự được đưa ra, đó là thứ duy nhất
trình duyệt từng nhìn thấy. `ca-status` cho biết chứng thực số là gì. Nếu chứng thực số không được
tin, `mix doctor --repair` là thứ đặt nó lại.

### "Đây là PHP nào, và vì sao?"

```bash
mix runtime resolve php
```

Phiên bản mà thư mục này nhận, **và cái nào trong bốn nguồn đã quyết định** — đó là nửa bạn cần khi
câu trả lời không phải cái bạn nghĩ.

## Đọc log

```bash
mix service logs caddy --follow
mix service logs mariadb@main -n 200
```

`--follow` sống sót qua việc service sập và được khởi động lại: thứ đang được theo dõi là service,
không phải một lần chạy của tiến trình. Log của chính daemon nằm ở `logs/daemon.log` trong home của
MixEngine.

Với một thao tác dài — một lần cài, một lần áp dụng blueprint — job là nơi cần nhìn:

```bash
mix job list
mix job status <id>
mix job logs <id>
```

`mix job logs` chỉ trả lời cho một job có chạy chương trình của người khác, mà hôm nay nghĩa là một
blueprint đang chạy câu lệnh dựng khung của chính nó. Mọi việc khác một job làm đều được báo cáo
dưới dạng tiến độ và kết quả, và lệnh này nói thẳng ra điều đó thay vì giả vờ rằng đầu ra đã mất.

## Vài tình huống thường gặp

**Một cổng đã bị chiếm.** Có thứ khác trên máy bạn đang giữ nó. `mix service create --port` chọn
cổng khác cho một service mới; với một service đã có, hãy xóa nó rồi tạo lại trên cổng khác — thư
mục dữ liệu được giữ lại.

**Daemon không khởi động được.** Hãy đọc `logs/daemon.log` trong home. `mix status --no-autostart`
hỏi xem có daemon nào đang chạy không mà không khởi động một cái, đó mới là câu hỏi đúng khi bạn
đang chẩn đoán chứ không phải đang làm việc.

**Một câu lệnh cần một phiên bản chưa được cài.** MixEngine nói ra điều đó và nêu chính xác câu lệnh
`mix runtime install` cần gõ. Khi thứ được hỏi là một *khoảng*, nó không thể biết phiên bản nào thỏa
mãn và trỏ sang `mix runtime available`.

**Có thứ xin quyền quản trị và bạn đã nói không.** Không có gì bị áp dụng dở dang. `mix elevation
status` cho thấy những gì còn đang chờ, và `mix elevation grant` hỏi lại.

## Báo lỗi

```bash
mix doctor --bundle
```

Một file nén chứa mọi thứ một báo cáo lỗi cần: những gì `doctor` tìm thấy, trạng thái của daemon
này, cỗ máy này là gì, và phần đuôi của log. `--out` chép nó sang nơi bạn chọn.

**Thứ nó cố ý bỏ ra ngoài được nêu tên ngay trong file nén**, để không ai phải đoán xem một phần
thiếu là bị lược bỏ hay là một thất bại. Hãy mở ra xem trước khi gửi đi đâu — nó là một file nén
bình thường, và nó là của bạn.

Mọi câu lệnh `mix` cũng nhận `--json`, và đó thường là cách nhanh nhất để cho ai đó thấy chính xác
thứ bạn đã thấy.
