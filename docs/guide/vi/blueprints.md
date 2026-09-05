+++
title = "Blueprint"
slug = "blueprints"
order = 9
summary = "Ghi lại một dự án được làm từ những gì, rồi dựng lại đúng như vậy ở nơi khác — hoặc trên máy của người khác."
translation_of = "en/blueprints.md"
source_sha256 = "7a79d2bf3edf50ce56c3181f920e99d3ac7422092bdce952584506b1f3227655"
+++

# Blueprint

Blueprint là bản ghi chép về việc một dự án được làm từ những gì: nó cần PHP nào, dùng những service
nào, site của nó trông ra sao, và tùy chọn thêm một câu lệnh dựng khung một bản mới. Đó là cách bạn
dựng cùng một môi trường hai lần — trên máy thứ hai, cho một đồng nghiệp, hoặc cho dự án tiếp theo
có cùng hình dáng.

## Chụp lại một cái

```bash
cd ~/code/blog
mix blueprint capture blog-stack --description "PHP 8.3, MariaDB, Redis"
mix blueprint list
```

Cái tên là thứ nó được lưu dưới đó — chữ thường, chữ số và dấu gạch nối.

**Blueprint mang theo hình dáng, không mang theo nội dung.** Nó ghi lại rằng dự án dùng một MariaDB
và phiên bản nào; nó không ghi lại dữ liệu của bạn, và không bao giờ chứa mật khẩu. Áp dụng một
blueprint cho bạn cùng một môi trường, không phải một bản sao công việc của bạn.

## Áp dụng một cái

```bash
mix blueprint apply blog-stack --project shop --dry-run
mix blueprint apply blog-stack --project shop
```

**Hãy chạy bản thử trước.** Nó in ra kế hoạch và không thay đổi gì: runtime nào sẽ được cài, service
nào sẽ được tạo, site sẽ tên gì, và — nếu có — câu lệnh dựng khung sẽ được chạy. Không có gì trong
một lần áp dụng bị giấu khỏi kế hoạch đó.

`--path` nói nơi dự án mới nằm; mặc định là một thư mục đặt tên theo dự án, nằm dưới chỗ bạn đang
đứng.

### Trả lời các câu hỏi về phiên bản

Một blueprint đòi PHP 8.3 trên máy đang có 8.2 là một câu hỏi, không phải một lỗi. Hai cờ trả lời
trước cho mọi câu hỏi loại đó trong kế hoạch:

| Cờ | Nghĩa là |
| --- | --- |
| `--install-missing` | Cài đúng thứ blueprint đòi |
| `--use-installed` | Dùng thứ máy này đã có |

## Nhận blueprint của người khác

```bash
mix blueprint import ./blog-stack.toml
```

Một blueprint đến từ nơi khác có thể mang theo một chữ ký rời — `mix` tìm `<file>.minisig` nằm cạnh
nó, hoặc nhận `--signature`. Và đây là quy tắc quan trọng:

**Thứ đến mà không có chữ ký được gallery bảo lãnh thì không đáng tin, vĩnh viễn.** Không gì nâng
điều đó lên về sau. Nhập lại nó kèm chữ ký cũng không tẩy trắng được; trạng thái tin cậy được quyết
định một lần, lúc nhập, và mọi danh sách nêu tên blueprint đó đều hiển thị nó.

Trạng thái đó không phải để trang trí. Nó quyết định câu lệnh `[scaffold]` của blueprint phải được
đồng ý lớn tiếng đến mức nào trước khi chạy.

## Câu lệnh dựng khung, và vì sao nó được hỏi

Một blueprint có thể mang theo một câu lệnh chạy một lần trong dự án mới — `composer create-project
…` hoặc thứ tương đương cho framework mà nó dành cho. Đó là chương trình của người khác chạy trên
máy bạn, nên MixEngine in ra chính xác câu lệnh đó và hỏi trước khi chạy, và nó hỏi khác nhau tùy
theo blueprint đến từ đâu.

Hai cờ bỏ qua câu hỏi, và **không cái nào bao được cái kia**:

| Cờ | Dành cho |
| --- | --- |
| `--run-scaffold` | Một blueprint mà gallery đã ký |
| `--run-untrusted-scaffold` | Một cái không đáng tin. Không gì bảo lãnh cho thứ này chạy |

Một script chạy câu lệnh chưa ký của ai đó thì nên nói ra điều ấy ngay trên dòng làm việc đó. Đó là
toàn bộ lý do có hai cờ chứ không phải một, và câu lệnh vẫn được in ra trước khi chạy trong cả hai
trường hợp.

## Theo dõi một lần áp dụng

Một lần áp dụng là một job. Nó có thể cài runtime, tạo service và chạy lệnh dựng khung, nên có thể
mất một lúc:

```bash
mix job list
mix job status <id>
mix job logs <id>
mix job wait <id>
```

`mix job logs` là nơi đầu ra của chính câu lệnh dựng khung đi tới — đó là thứ duy nhất một lần áp
dụng chạy mà tự in ra cái gì. Các dòng đó sống chừng nào daemon còn giữ job, nên nó là thứ để đọc
trong lúc job đang chạy chứ không phải một bản ghi để quay lại xem vào tuần sau.

Nếu lần áp dụng cần quyền quản trị — chẳng hạn một tên miền mới cần được định tuyến — nó hỏi một lần
ở cuối. `--grant` tiêu luôn hộp thoại đó mà không hỏi trước.
