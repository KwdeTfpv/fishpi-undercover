# 词语库管理指南

## 概述

卧底游戏的词语库系统支持分类管理、难度等级和相似度评分，让游戏更加丰富有趣。

## 文件结构

```
data/
└── words.json          # 词语库主文件
```

## 词语库格式

词语库使用JSON格式，按分类组织：

```json
{
  "categories": {
    "分类名称": [
      {
        "civilian_word": "平民词语",
        "undercover_word": "卧底词语", 
        "similarity": 0.8,
        "difficulty": "easy"
      }
    ]
  }
}
```

### 字段说明

- `civilian_word`: 平民看到的词语
- `undercover_word`: 卧底看到的词语
- `similarity`: 相似度评分 (0.0-1.0)
- `difficulty`: 难度等级 (easy/medium/hard)

## 使用词语管理工具

### 安装

```bash
cargo build --release
```

### 查看词库

```bash
./target/release/word-manager list
```

### 添加新词对

```bash
./target/release/word-manager add "食物" "苹果" "梨" 0.8 easy
```

### 查看统计信息

```bash
./target/release/word-manager stats
```

### 验证词库

```bash
./target/release/word-manager validate
```

### 导出词库

```bash
./target/release/word-manager export backup.json
```

## 配置选项

在 `config.toml` 中配置词语库：

```toml
[word_bank]
file_path = "data/words.json"      # 词库文件路径
min_similarity = 0.5               # 最小相似度
max_words_per_category = 10        # 每分类最大词数
enable_categories = true           # 启用分类功能
```

## 编程接口

### 基本使用

```rust
use fishpi_undercover::word_bank::WordBank;

let word_bank = WordBank::new();

// 获取随机词对
if let Some(word_pair) = word_bank.get_random_word_pair() {
    println!("平民: {}, 卧底: {}", 
        word_pair.civilian_word, 
        word_pair.undercover_word);
}
```

### 按分类获取

```rust
// 从指定分类获取词对
if let Some(word_pair) = word_bank.get_word_pair_from_category("食物") {
    // 使用词对
}
```

### 按难度获取

```rust
use fishpi_undercover::word_bank::Difficulty;

// 获取简单难度的词对
if let Some(word_pair) = word_bank.get_word_pair_by_difficulty(Difficulty::Easy) {
    // 使用词对
}
```

### 按相似度获取

```rust
// 获取相似度大于0.7的词对
if let Some(word_pair) = word_bank.get_word_pair_by_similarity(0.7) {
    // 使用词对
}
```

## 最佳实践

### 1. 相似度评分

- **0.9-1.0**: 非常相似，游戏难度很高
- **0.7-0.8**: 相似，适合一般游戏
- **0.5-0.6**: 中等相似，平衡的游戏体验
- **0.3-0.4**: 差异较大，游戏较简单

### 2. 难度设计

- **Easy**: 新手友好，词语常见易懂
- **Medium**: 中等难度，需要一定思考
- **Hard**: 高难度，需要深入分析

### 3. 分类建议

- 食物类：苹果-梨、米饭-面条
- 电子产品：手机-平板、电脑-笔记本
- 动物类：猫-狗、老虎-狮子
- 职业类：医生-护士、老师-教授
- 运动类：足球-篮球、游泳-潜水

## 扩展词库

### 手动添加

1. 编辑 `data/words.json` 文件
2. 按照JSON格式添加新词对
3. 重启游戏服务器

### 使用工具添加

```bash
# 批量添加示例
./target/release/word-manager add "娱乐" "电影" "电视剧" 0.8 easy
./target/release/word-manager add "娱乐" "游戏" "玩具" 0.6 medium
./target/release/word-manager add "娱乐" "音乐" "歌曲" 0.8 easy
```

## 故障排除

### 词库文件不存在

如果词库文件不存在，系统会自动创建默认词库。

### 格式错误

使用验证命令检查词库格式：

```bash
./target/release/word-manager validate
```

### 性能优化

- 词库文件不要过大（建议<1MB）
- 定期清理重复或低质量词对
- 使用合适的相似度评分

## 贡献指南

欢迎贡献新的词对！请遵循以下规范：

1. 确保词语适合所有年龄段
2. 避免政治敏感或争议性词语
3. 提供合理的相似度评分
4. 选择合适的分类和难度 